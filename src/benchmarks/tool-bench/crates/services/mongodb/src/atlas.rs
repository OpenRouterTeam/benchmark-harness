//! Atlas Admin API tools (`atlas-*`), answered from the dataset's orgs, projects, clusters,
//! database users, access lists, alerts and performance-advisor documents.

use serde_json::{Map, Value, json};

use crate::db::{client_ip, slugify, suffix};
use crate::{Ctx, error_running, fail, ok, str_arg, text};

const REGIONS: &str = include_str!("../assets/atlas-regions.json");
const SIZES: [&str; 7] = ["M10", "M20", "M30", "M40", "M50", "M60", "M80"];

const ACCESS_LIST_ADDED_NOTE: &str = "Note: Your current IP address has been added to the Atlas project's IP access list to enable secure connection.";
const DEFAULT_ACCESS_LIST_COMMENT: &str = "Added by MongoDB MCP Server to enable tool access";
const CREATED_USER_MESSAGE: &str = "Note: A temporary user has been created to enable secure connection to the cluster. For more information, see https://dochub.mongodb.org/core/mongodb-mcp-server-tools-considerations\n\nNote to LLM Agent: it is important to include the following link in your response to the user in case they want to get more information about the temporary user created: https://dochub.mongodb.org/core/mongodb-mcp-server-tools-considerations";
const SUGGESTED_INDEXES_COPY: &str = "Note: The \"Weight\" field is measured in bytes, and represents the estimated number of bytes saved in disk reads per executed read query that would be saved by implementing an index suggestion. Please convert this to MB or GB for easier readability.";
const SLOW_QUERY_LOGS_COPY: &str = "Please notify the user that the MCP server tool limits slow query logs to the most recent 50 slow query logs. This is a limitation of the MCP server tool only. More slow query logs and performance suggestions can be seen in the Atlas UI. Please give to the user the following docs about the performance advisor: https://www.mongodb.com/docs/atlas/performance-advisor/.";

/// An Atlas API failure, rendered the way `ApiClientError` formats it.
struct ApiError {
    status: u16,
    reason: &'static str,
    detail: String,
}

impl ApiError {
    fn not_found(detail: String) -> Self {
        Self {
            status: 404,
            reason: "Not Found",
            detail,
        }
    }
    fn bad_request(detail: String) -> Self {
        Self {
            status: 400,
            reason: "Bad Request",
            detail,
        }
    }
    fn conflict(detail: String) -> Self {
        Self {
            status: 409,
            reason: "Conflict",
            detail,
        }
    }
    fn message(&self) -> String {
        format!(
            "[{} {}] error calling Atlas API: {}; {}",
            self.status, self.reason, self.reason, self.detail
        )
    }
}

enum Failure {
    Api(ApiError),
    /// A tool-specific error rendered verbatim.
    Plain(String),
    /// `Error running <tool>: <message>`.
    Running(String),
}

impl From<ApiError> for Failure {
    fn from(e: ApiError) -> Self {
        Failure::Api(e)
    }
}

type R = Result<Value, Failure>;

pub fn call(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    refresh_clusters(ctx);
    let result = match name {
        "atlas-list-orgs" => list_orgs(ctx, args),
        "atlas-list-projects" => list_projects(ctx, args),
        "atlas-create-project" => create_project(ctx, args),
        "atlas-list-clusters" => list_clusters(ctx, args),
        "atlas-inspect-cluster" => inspect_cluster(ctx, args),
        "atlas-create-free-cluster" => create_free_cluster(ctx, args),
        "atlas-create-cluster" => create_cluster(ctx, args),
        "atlas-pause-resume-cluster" => pause_resume(ctx, args),
        "atlas-upgrade-cluster" => upgrade_cluster(ctx, args),
        "atlas-connect-cluster" => connect_cluster(ctx, args),
        "atlas-create-access-list" => create_access_list(ctx, args),
        "atlas-inspect-access-list" => inspect_access_list(ctx, args),
        "atlas-list-db-users" => list_db_users(ctx, args),
        "atlas-create-db-user" => create_db_user(ctx, args),
        "atlas-list-alerts" => list_alerts(ctx, args),
        "atlas-get-performance-advisor" => performance_advisor(ctx, args),
        "atlas-get-regions" => get_regions(args),
        "atlas-load-sample-dataset" => load_sample_dataset(ctx, args),
        _ => Err(Failure::Running(format!("{name} is not implemented"))),
    };
    match result {
        Ok(v) => v,
        Err(Failure::Api(e)) => error_running(name, &e.message()),
        Err(Failure::Plain(m)) => fail(m),
        Err(Failure::Running(m)) => error_running(name, &m),
    }
}

fn s<'a>(args: &'a Value, key: &str) -> &'a str {
    str_arg(args, key).unwrap_or_default()
}

fn project(ctx: &Ctx, id: &str) -> Result<Value, ApiError> {
    ctx.coll("projects")
        .and_then(|c| c.get(id))
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("No group with ID {id} exists.")))
}

fn cluster_key(project_id: &str, name: &str) -> String {
    format!("{project_id}/{name}")
}

fn cluster(ctx: &Ctx, project_id: &str, name: &str) -> Result<Value, ApiError> {
    project(ctx, project_id)?;
    ctx.coll("clusters")
        .and_then(|c| c.get(&cluster_key(project_id, name)))
        .cloned()
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "No cluster named {name} exists in group {project_id}."
            ))
        })
}

fn project_clusters(ctx: &Ctx, project_id: &str) -> Vec<Value> {
    ctx.docs("clusters")
        .into_iter()
        .filter(|c| c["groupId"] == project_id)
        .collect()
}

fn is_flex(c: &Value) -> bool {
    c["providerSettings"]["providerName"] == "FLEX"
}

/// Clusters being created or updated become IDLE once logical time has moved on.
fn refresh_clusters(ctx: &mut Ctx) {
    let now = ctx.now;
    let keys: Vec<String> = ctx
        .coll("clusters")
        .map(|c| {
            c.iter()
                .filter(|(_, v)| v["_readyAt"].as_i64().is_some_and(|t| t <= now))
                .map(|(k, _)| k.to_owned())
                .collect()
        })
        .unwrap_or_default();
    for k in keys {
        if let Some(c) = ctx.coll_mut("clusters").get_mut(&k) {
            c["stateName"] = json!("IDLE");
            if let Some(m) = c.as_object_mut() {
                m.shift_remove("_readyAt");
            }
        }
    }
}

fn process_ids(cs: &Value) -> Vec<Value> {
    let Some(standard) = cs["standard"].as_str() else {
        return vec![];
    };
    let rest = standard
        .strip_prefix("mongodb://")
        .or_else(|| standard.strip_prefix("mongodb+srv://"))
        .unwrap_or(standard);
    let hosts = rest.split('/').next().unwrap_or("");
    let hosts = hosts.rsplit('@').next().unwrap_or("");
    hosts
        .split(',')
        .filter(|h| !h.is_empty())
        .map(|h| {
            if h.contains(':') {
                json!(h)
            } else {
                json!(format!("{h}:27017"))
            }
        })
        .collect()
}

struct ClusterInfo {
    name: String,
    id: Value,
    instance_type: &'static str,
    instance_size: Option<String>,
    provider: Option<String>,
    region: Option<String>,
    paused: bool,
    state: Value,
    version: Value,
    connection_strings: Value,
}

fn info(c: &Value) -> ClusterInfo {
    if is_flex(c) {
        return ClusterInfo {
            name: c["name"].as_str().unwrap_or("").to_owned(),
            id: c["id"].clone(),
            instance_type: "FLEX",
            instance_size: None,
            provider: c["providerSettings"]["backingProviderName"]
                .as_str()
                .map(str::to_owned),
            region: c["providerSettings"]["regionName"]
                .as_str()
                .map(str::to_owned),
            paused: false,
            state: c["stateName"].clone(),
            version: c["mongoDBVersion"].clone(),
            connection_strings: c["connectionStrings"].clone(),
        };
    }
    let rc = &c["replicationSpecs"][0]["regionConfigs"][0];
    let size = rc["electableSpecs"]["instanceSize"]
        .as_str()
        .or_else(|| rc["readOnlySpecs"]["instanceSize"].as_str())
        .unwrap_or("UNKNOWN")
        .to_owned();
    let free = size == "M0";
    ClusterInfo {
        name: c["name"].as_str().unwrap_or("").to_owned(),
        id: c["id"].clone(),
        instance_type: if free { "FREE" } else { "DEDICATED" },
        instance_size: (!free).then_some(size),
        provider: if free {
            rc["backingProviderName"].as_str()
        } else {
            rc["providerName"].as_str()
        }
        .map(str::to_owned),
        region: rc["regionName"].as_str().map(str::to_owned),
        paused: c["paused"].as_bool().unwrap_or(false),
        state: c["stateName"].clone(),
        version: c["mongoDBVersion"].clone(),
        connection_strings: c["connectionStrings"].clone(),
    }
}

fn insert_some(m: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v.filter(|v| !v.is_null()) {
        m.insert(k.to_owned(), v);
    }
}

fn format_cluster(c: &Value) -> Value {
    let i = info(c);
    let mut m = Map::new();
    m.insert("name".to_owned(), json!(i.name));
    insert_some(&mut m, "clusterId", Some(i.id));
    m.insert("instanceType".to_owned(), json!(i.instance_type));
    insert_some(&mut m, "instanceSize", i.instance_size.map(Value::String));
    insert_some(&mut m, "provider", i.provider.map(Value::String));
    insert_some(&mut m, "region", i.region.map(Value::String));
    m.insert("paused".to_owned(), json!(i.paused));
    insert_some(&mut m, "state", Some(i.state));
    insert_some(&mut m, "mongoDBVersion", Some(i.version));
    insert_some(
        &mut m,
        "connectionStrings",
        Some(i.connection_strings.clone()),
    );
    m.insert(
        "processIds".to_owned(),
        Value::Array(process_ids(&i.connection_strings)),
    );
    Value::Object(m)
}

fn page(items: Vec<Value>, args: &Value) -> (Vec<Value>, usize) {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .max(1) as usize;
    let page_num = args
        .get("pageNum")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let total = items.len();
    (
        items
            .into_iter()
            .skip((page_num - 1) * limit)
            .take(limit)
            .collect(),
        total,
    )
}

fn list_orgs(ctx: &mut Ctx, args: &Value) -> R {
    let include_count = args
        .get("includeCount")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let page_num = args.get("pageNum").and_then(Value::as_u64).unwrap_or(1) as usize;
    let (orgs, total) = page(ctx.docs("orgs"), args);
    let orgs: Vec<Value> = orgs
        .iter()
        .map(|o| json!({ "name": o["name"], "id": o["id"] }))
        .collect();
    if orgs.is_empty() {
        return Ok(ok(
            vec![text(
                "No organizations found in your MongoDB Atlas account.",
            )],
            Some(json!({ "organizations": [], "totalCount": 0 })),
        ));
    }
    let more = if include_count && total > 0 {
        (page_num - 1) * limit + orgs.len() < total
    } else {
        orgs.len() == limit
    };
    let total_count = if include_count && total > 0 {
        total
    } else {
        orgs.len()
    };
    let content = ctx.untrusted(
        &format!(
            "Found {} organizations in your MongoDB Atlas account.{}",
            orgs.len(),
            if more {
                " Use pagination arguments if more results are expected."
            } else {
                ""
            }
        ),
        vec![Value::Array(orgs.clone()).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "organizations": orgs, "totalCount": total_count })),
    ))
}

/// `new Date(x).toLocaleString()` in en-US, UTC (the tools container's locale).
fn locale_string(iso: &str) -> String {
    let Some(ms) = crate::ejson::parse_iso_millis(iso) else {
        return "Invalid Date".to_owned();
    };
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp(ms / 1000) else {
        return "Invalid Date".to_owned();
    };
    let (h, ampm) = match dt.hour() {
        0 => (12, "AM"),
        h @ 1..=11 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!(
        "{}/{}/{}, {}:{:02}:{:02} {}",
        u8::from(dt.month()),
        dt.day(),
        dt.year(),
        h,
        dt.minute(),
        dt.second(),
        ampm
    )
}

fn list_projects(ctx: &mut Ctx, args: &Value) -> R {
    let org = str_arg(args, "orgId");
    if let Some(org) = org
        && !ctx.coll("orgs").is_some_and(|c| c.contains(org))
    {
        return Err(ApiError::not_found(format!("No organization with ID {org} exists.")).into());
    }
    let include_count = args
        .get("includeCount")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let page_num = args.get("pageNum").and_then(Value::as_u64).unwrap_or(1) as usize;
    let all: Vec<Value> = ctx
        .docs("projects")
        .into_iter()
        .filter(|p| org.is_none_or(|o| p["orgId"] == o))
        .collect();
    let (items, total) = page(all, args);
    let projects: Vec<Value> = items
        .iter()
        .map(|p| {
            json!({
                "name": p["name"],
                "id": p["id"],
                "orgId": p["orgId"],
                "created": p["created"].as_str().map_or("N/A".to_owned(), locale_string),
            })
        })
        .collect();
    let mut structured = Map::new();
    if let Some(o) = org {
        structured.insert("orgId".to_owned(), json!(o));
    }
    if projects.is_empty() {
        structured.insert("projects".to_owned(), json!([]));
        structured.insert("totalCount".to_owned(), json!(0));
        let msg = match org {
            Some(o) => format!("No projects found in organization {o}."),
            None => "No projects found in your MongoDB Atlas account.".to_owned(),
        };
        return Ok(ok(vec![text(msg)], Some(Value::Object(structured))));
    }
    let more = if include_count && total > 0 {
        (page_num - 1) * limit + projects.len() < total
    } else {
        projects.len() == limit
    };
    let total_count = if include_count && total > 0 {
        total
    } else {
        projects.len()
    };
    let content = ctx.untrusted(
        &format!(
            "Found {} projects.{}",
            projects.len(),
            if more {
                " Use pagination arguments if more results are expected."
            } else {
                ""
            }
        ),
        vec![serde_json::to_string_pretty(&projects).expect("serializes")],
    );
    structured.insert("projects".to_owned(), Value::Array(projects));
    structured.insert("totalCount".to_owned(), json!(total_count));
    Ok(ok(content, Some(Value::Object(structured))))
}

fn create_project(ctx: &mut Ctx, args: &Value) -> R {
    let name = s(args, "projectName").to_owned();
    let org = s(args, "orgId").to_owned();
    if !ctx.coll("orgs").is_some_and(|c| c.contains(&org)) {
        return Err(ApiError::not_found(format!("No organization with ID {org} exists.")).into());
    }
    if ctx
        .docs("projects")
        .iter()
        .any(|p| p["name"] == name.as_str() && p["orgId"] == org.as_str())
    {
        return Err(
            ApiError::conflict(format!("A group with name \"{name}\" already exists.")).into(),
        );
    }
    let id = ctx.object_id("atlas-project");
    let created = ctx.now_iso();
    ctx.coll_mut("projects").insert(
        id.clone(),
        json!({ "id": id, "name": name, "orgId": org, "created": created, "clusterCount": 0, "regionUsageRestrictions": "COMMERCIAL_FEDRAMP_REGIONS_ONLY", "withDefaultAlertsSettings": true }),
    );
    Ok(ok(
        vec![text(format!("Project \"{name}\" created successfully."))],
        Some(json!({ "projectName": name, "orgId": org })),
    ))
}

fn list_clusters(ctx: &mut Ctx, args: &Value) -> R {
    match str_arg(args, "projectId") {
        None => {
            let mut rows = Vec::new();
            for p in ctx.docs("projects") {
                for c in project_clusters(ctx, p["id"].as_str().unwrap_or("")) {
                    rows.push(json!({ "projectName": p["name"], "projectId": p["id"], "clusterName": c["name"] }));
                }
            }
            if rows.is_empty() {
                return Ok(ok(
                    vec![text("No clusters found.")],
                    Some(json!({ "clusters": [], "totalCount": 0 })),
                ));
            }
            let content = ctx.untrusted(
                &format!("Found {} clusters across all projects", rows.len()),
                vec![Value::Array(rows.clone()).to_string()],
            );
            Ok(ok(
                content,
                Some(json!({ "clusters": rows, "totalCount": rows.len() })),
            ))
        }
        Some(pid) => {
            let p = project(ctx, pid)?;
            let clusters = project_clusters(ctx, pid);
            let mut formatted: Vec<Value> = clusters
                .iter()
                .filter(|c| !is_flex(c))
                .map(format_cluster)
                .collect();
            formatted.extend(clusters.iter().filter(|c| is_flex(c)).map(format_cluster));
            if formatted.is_empty() {
                return Ok(ok(
                    vec![text("No clusters found.")],
                    Some(
                        json!({ "projectId": p["id"], "projectName": p["name"], "clusters": [], "totalCount": 0 }),
                    ),
                ));
            }
            let content = ctx.untrusted(
                &format!("Found {} clusters in project {pid}:", formatted.len()),
                vec![json!({ "projectName": p["name"], "clusters": formatted }).to_string()],
            );
            Ok(ok(
                content,
                Some(
                    json!({ "projectId": p["id"], "projectName": p["name"], "clusters": formatted, "totalCount": formatted.len() }),
                ),
            ))
        }
    }
}

fn inspect_cluster(ctx: &mut Ctx, args: &Value) -> R {
    let c = cluster(ctx, s(args, "projectId"), s(args, "clusterName"))?;
    let i = info(&c);
    let mut m = Map::new();
    m.insert(
        "name".to_owned(),
        json!(if i.name.is_empty() {
            "Unknown".to_owned()
        } else {
            i.name
        }),
    );
    m.insert("instanceType".to_owned(), json!(i.instance_type));
    m.insert(
        "instanceSize".to_owned(),
        json!(i.instance_size.unwrap_or_else(|| "N/A".to_owned())),
    );
    insert_some(&mut m, "provider", i.provider.map(Value::String));
    insert_some(&mut m, "region", i.region.map(Value::String));
    m.insert("paused".to_owned(), json!(i.paused));
    m.insert(
        "state".to_owned(),
        if i.state.is_string() {
            i.state
        } else {
            json!("UNKNOWN")
        },
    );
    m.insert(
        "mongoDBVersion".to_owned(),
        if i.version.is_string() {
            i.version
        } else {
            json!("N/A")
        },
    );
    m.insert(
        "connectionStrings".to_owned(),
        if i.connection_strings.is_object() {
            i.connection_strings
        } else {
            json!({})
        },
    );
    let structured = Value::Object(m);
    let content = ctx.untrusted("Cluster details:", vec![structured.to_string()]);
    Ok(ok(content, Some(structured)))
}

/// `ensureCurrentIpInAccessList`: adds the caller's IP unless it is already allowed.
fn ensure_current_ip(ctx: &mut Ctx, project_id: &str) -> &'static str {
    let ip = client_ip(ctx);
    let key = format!("{project_id}/{ip}/32");
    if ctx.coll("access_list").is_some_and(|c| c.contains(&key)) {
        return "already-present";
    }
    ctx.coll_mut("access_list").insert(
        key,
        json!({ "groupId": project_id, "ipAddress": ip, "cidrBlock": format!("{ip}/32"), "comment": DEFAULT_ACCESS_LIST_COMMENT }),
    );
    "added"
}

fn note(result: &str) -> Vec<Value> {
    if result == "added" {
        vec![text(ACCESS_LIST_ADDED_NOTE)]
    } else {
        vec![]
    }
}

fn host_hash(ctx: &mut Ctx) -> String {
    ctx.svc
        .ids
        .next_prefixed("cluster-host", "", 5)
        .to_lowercase()
}

fn new_cluster(
    ctx: &mut Ctx,
    project_id: &str,
    name: &str,
    region_configs: Value,
    cluster_type: &str,
    extra: Value,
) -> Value {
    let id = ctx.object_id("atlas-cluster");
    let hash = host_hash(ctx);
    let slug = slugify(name);
    let created = ctx.now_iso();
    let mut c = json!({
        "id": id,
        "groupId": project_id,
        "name": name,
        "clusterType": cluster_type,
        "createDate": created,
        "mongoDBMajorVersion": "8.0",
        "mongoDBVersion": "8.0.15",
        "paused": false,
        "pitEnabled": false,
        "backupEnabled": false,
        "terminationProtectionEnabled": false,
        "versionReleaseSystem": "LTS",
        "stateName": "CREATING",
        "replicationSpecs": [{ "zoneName": "Zone 1", "regionConfigs": region_configs }],
        "connectionStrings": {
            "standard": format!("mongodb://{slug}-shard-00-00.{hash}.mongodb.net:27017,{slug}-shard-00-01.{hash}.mongodb.net:27017,{slug}-shard-00-02.{hash}.mongodb.net:27017/?ssl=true&authSource=admin&replicaSet=atlas-{hash}-shard-0"),
            "standardSrv": format!("mongodb+srv://{slug}.{hash}.mongodb.net"),
        },
        "_readyAt": ctx.now + 1,
    });
    if let (Some(m), Value::Object(e)) = (c.as_object_mut(), extra) {
        for (k, v) in e {
            m.insert(k, v);
        }
    }
    c
}

fn create_free_cluster(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "name").to_owned();
    let region = str_arg(args, "region").unwrap_or("US_EAST_1").to_owned();
    project(ctx, &pid)?;
    let ip = ensure_current_ip(ctx, &pid);
    if ctx
        .coll("clusters")
        .is_some_and(|c| c.contains(&cluster_key(&pid, &name)))
    {
        return Err(ApiError::bad_request(format!(
            "Cluster {name} already exists in group {pid}."
        ))
        .into());
    }
    if project_clusters(ctx, &pid)
        .iter()
        .any(|c| info(c).instance_type == "FREE")
    {
        return Err(ApiError::bad_request(
            "Cannot create more than 1 free cluster per project.".to_owned(),
        )
        .into());
    }
    let rc = json!([{ "providerName": "TENANT", "backingProviderName": "AWS", "regionName": region, "priority": 7, "electableSpecs": { "instanceSize": "M0" } }]);
    let c = new_cluster(ctx, &pid, &name, rc, "REPLICASET", json!({}));
    ctx.coll_mut("clusters").insert(cluster_key(&pid, &name), c);
    let mut content = vec![text(format!(
        "Cluster \"{name}\" has been created in region \"{region}\"."
    ))];
    content.extend(note(ip));
    Ok(ok(content, Some(json!({ "created": true }))))
}

fn max_autoscaling(size: &str, provider: &str) -> String {
    match size {
        "M80" => "M200".to_owned(),
        "M60" => {
            if provider == "AZURE" {
                "M200".to_owned()
            } else {
                "M140".to_owned()
            }
        }
        s => {
            let i = SIZES.iter().position(|x| *x == s).unwrap_or(0);
            SIZES.get(i + 2).unwrap_or(&"M80").to_string()
        }
    }
}

fn create_cluster(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "clusterName").to_owned();
    let provider = s(args, "provider").to_owned();
    let mut regions: Vec<String> = args["regions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if regions.is_empty()
        && let Some(r) = str_arg(args, "region")
    {
        regions.push(r.to_owned());
    }
    let cluster_type = str_arg(args, "clusterType")
        .unwrap_or("REPLICASET")
        .to_owned();
    let size_arg = str_arg(args, "instanceSize");
    if cluster_type == "SHARDED" && matches!(size_arg, Some("M10" | "M20")) {
        return Err(Failure::Plain(
            "SHARDED clusters require M30 or higher instance size.".to_owned(),
        ));
    }
    project(ctx, &pid)?;
    let size = match size_arg {
        Some(sz) => sz.to_owned(),
        None if cluster_type == "SHARDED" => "M30".to_owned(),
        None => {
            if project_clusters(ctx, &pid).len() < 2 {
                "M10".to_owned()
            } else {
                "M30".to_owned()
            }
        }
    };
    if ctx
        .coll("clusters")
        .is_some_and(|c| c.contains(&cluster_key(&pid, &name)))
    {
        return Err(ApiError::bad_request(format!(
            "Cluster {name} already exists in group {pid}."
        ))
        .into());
    }
    let compute = args
        .get("computeAutoScaling")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let ear = str_arg(args, "encryptionAtRestProvider")
        .unwrap_or("NONE")
        .to_owned();
    let ip = ensure_current_ip(ctx, &pid);
    let rcs: Vec<Value> = regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "providerName": provider,
                "regionName": r,
                "priority": 7 - i as i64,
                "electableSpecs": { "instanceSize": size, "nodeCount": if regions.len() == 1 { 3 } else if i == 0 { 2 } else { 1 } },
                "autoScaling": {
                    "compute": if compute { json!({ "enabled": true, "scaleDownEnabled": true, "minInstanceSize": size, "maxInstanceSize": max_autoscaling(&size, &provider) }) } else { json!({ "enabled": false, "scaleDownEnabled": false }) },
                    "diskGB": { "enabled": true },
                },
            })
        })
        .collect();
    let extra = json!({
        "backupEnabled": str_arg(args, "backup").unwrap_or("SNAPSHOT") != "OFF",
        "pitEnabled": str_arg(args, "backup") == Some("CONTINUOUS"),
        "terminationProtectionEnabled": args.get("terminationProtectionEnabled").and_then(Value::as_bool).unwrap_or(false),
        "encryptionAtRestProvider": ear,
        "diskSizeGB": args.get("diskSizeGB").cloned().unwrap_or(json!(10)),
    });
    let c = new_cluster(ctx, &pid, &name, Value::Array(rcs), &cluster_type, extra);
    let id = c["id"].clone();
    ctx.coll_mut("clusters").insert(cluster_key(&pid, &name), c);
    let mut content = vec![text(format!(
        "Cluster \"{name}\" is being created in project \"{pid}\" ({size} {cluster_type} on {provider}/{}). Use the atlas-inspect-cluster tool with projectId \"{pid}\" and clusterName \"{name}\" to poll for readiness. The cluster is ready when its state is IDLE, connection strings are unavailable until then.",
        regions.join(", ")
    ))];
    if ip == "added" {
        content.push(text(ACCESS_LIST_ADDED_NOTE));
    }
    let mut structured = json!({ "clusterId": id, "provider": provider, "regions": regions, "instanceSize": size, "clusterType": cluster_type, "encryptionAtRestProvider": ear });
    for (k, default) in [
        ("mongoDBVersion", json!("LATEST")),
        ("backup", json!("SNAPSHOT")),
        ("computeAutoScaling", json!(true)),
        ("terminationProtectionEnabled", json!(false)),
        ("diskSizeGB", Value::Null),
    ] {
        match args.get(k).filter(|v| !v.is_null()) {
            Some(v) => structured[k] = v.clone(),
            None if !default.is_null() => structured[k] = default,
            None => {}
        }
    }
    Ok(ok(content, Some(structured)))
}

fn pause_resume(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "clusterName").to_owned();
    let action = s(args, "action").to_owned();
    let c = cluster(ctx, &pid, &name)?;
    let i = info(&c);
    let pause = action == "PAUSE";
    if i.instance_type != "DEDICATED" {
        return Err(ApiError::bad_request(format!(
            "Cannot update cluster {name}: {} clusters cannot be paused or resumed.",
            if i.instance_type == "FREE" {
                "Free"
            } else {
                "Flex"
            }
        ))
        .into());
    }
    if pause && i.paused {
        return Err(ApiError::bad_request(format!("Cluster {name} is already paused.")).into());
    }
    if pause && c["stateName"] != "IDLE" {
        return Err(ApiError::bad_request(format!(
            "Cluster {name} cannot be paused while it is in state {}.",
            c["stateName"].as_str().unwrap_or("UNKNOWN")
        ))
        .into());
    }
    let key = cluster_key(&pid, &name);
    if let Some(doc) = ctx.coll_mut("clusters").get_mut(&key)
        && (pause || i.paused)
    {
        doc["paused"] = json!(pause);
    }
    let mut disconnected = Vec::new();
    let text_body = if pause {
        let affected: Vec<String> = ctx
            .docs("connections")
            .iter()
            .filter(|e| {
                e["atlasCluster"]["projectId"] == pid.as_str()
                    && e["atlasCluster"]["clusterName"] == name.as_str()
            })
            .filter_map(|e| e["connectionId"].as_str().map(str::to_owned))
            .collect();
        for id in &affected {
            if let Some(entry) = ctx.coll_mut("connections").remove(id)
                && let Some(u) = entry["temporaryUser"].as_str()
            {
                ctx.coll_mut("db_users").remove(u);
            }
            disconnected.push(id.clone());
        }
        let mut t = format!(
            "Cluster \"{name}\" in project \"{pid}\" is being paused. Paused clusters are unavailable for connections and do not incur compute costs."
        );
        if !disconnected.is_empty() {
            t.push_str(&format!(
                " The following connections to cluster \"{name}\" were disconnected and their connectionIds are no longer valid: {}.",
                disconnected.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(", ")
            ));
        }
        t
    } else {
        format!(
            "Cluster \"{name}\" in project \"{pid}\" is being resumed. Use the atlas-inspect-cluster tool with projectId \"{pid}\" and clusterName \"{name}\" to poll for readiness. The cluster is ready when its state is IDLE."
        )
    };
    Ok(ok(
        vec![text(text_body)],
        Some(
            json!({ "clusterName": name, "action": action, "clusterId": c["id"], "disconnectedConnectionIds": disconnected }),
        ),
    ))
}

fn upgrade_cluster(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "clusterName").to_owned();
    let c = cluster(ctx, &pid, &name)?;
    let i = info(&c);
    let key = cluster_key(&pid, &name);
    let target = str_arg(args, "targetTier");
    let compute_arg = args.get("computeAutoScaling").and_then(Value::as_bool);
    let min_arg = str_arg(args, "minInstanceSize");
    let max_arg = str_arg(args, "maxInstanceSize");
    let autoscaling_args = compute_arg.is_some() || min_arg.is_some() || max_arg.is_some();
    let provider = str_arg(args, "provider")
        .map(str::to_owned)
        .or(i.provider.clone())
        .unwrap_or_else(|| "AWS".to_owned());
    let region = str_arg(args, "region")
        .map(str::to_owned)
        .or(i.region.clone())
        .unwrap_or_else(|| "US_EAST_1".to_owned());
    if i.instance_type == "DEDICATED" {
        if target == Some("FLEX") {
            return Err(Failure::Plain(format!(
                "Cluster \"{name}\" is already Dedicated. targetTier must be an instance size (M10-M80) to scale it in place, not FLEX."
            )));
        }
        if args.get("provider").is_some() || args.get("region").is_some() {
            return Err(Failure::Plain(format!(
                "Invalid Arguments:provider/region. provider/region are not valid when scaling an already-Dedicated cluster \"{name}\"."
            )));
        }
        if target.is_none() && !autoscaling_args {
            return Err(Failure::Plain(format!(
                "No changes specified for Dedicated cluster \"{name}\". Provide targetTier (new instance size), computeAutoScaling, minInstanceSize, and/or maxInstanceSize to scale it."
            )));
        }
        let current = i.instance_size.clone().unwrap_or_default();
        let new_size = target.map_or(current.clone(), str::to_owned);
        let scaling = new_size != current;
        let cur_auto = &c["replicationSpecs"][0]["regionConfigs"][0]["autoScaling"]["compute"];
        let enabled = compute_arg.unwrap_or_else(|| cur_auto["enabled"].as_bool().unwrap_or(false));
        let min = min_arg.map(str::to_owned).or_else(|| {
            if !enabled {
                None
            } else if scaling {
                Some(new_size.clone())
            } else {
                cur_auto["minInstanceSize"]
                    .as_str()
                    .map(str::to_owned)
                    .or(Some(new_size.clone()))
            }
        });
        let max = max_arg.map(str::to_owned).or_else(|| {
            if !enabled {
                None
            } else if scaling {
                Some(max_autoscaling(&new_size, &provider))
            } else {
                cur_auto["maxInstanceSize"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| Some(max_autoscaling(&new_size, &provider)))
            }
        });
        let ready_at = ctx.now + 1;
        if let Some(doc) = ctx.coll_mut("clusters").get_mut(&key) {
            if let Some(rcs) = doc["replicationSpecs"][0]["regionConfigs"].as_array_mut() {
                for rc in rcs {
                    rc["electableSpecs"]["instanceSize"] = json!(new_size);
                    rc["autoScaling"]["compute"] = match (&min, &max) {
                        (Some(mn), Some(mx)) if enabled => {
                            json!({ "enabled": true, "scaleDownEnabled": true, "minInstanceSize": mn, "maxInstanceSize": mx })
                        }
                        _ => json!({ "enabled": false, "scaleDownEnabled": false }),
                    };
                }
            }
            doc["stateName"] = json!("UPDATING");
            doc["_readyAt"] = json!(ready_at);
        }
        let summary = if enabled {
            format!(
                "compute autoscaling enabled ({}-{})",
                min.clone().unwrap_or_default(),
                max.clone().unwrap_or_default()
            )
        } else {
            "compute autoscaling disabled".to_owned()
        };
        let t = match (scaling, autoscaling_args) {
            (true, true) => format!(
                "Cluster \"{name}\" is being scaled to {new_size} with {summary}. This may take a few minutes."
            ),
            (true, false) => format!(
                "Cluster \"{name}\" is being scaled to {new_size}. This may take a few minutes."
            ),
            (false, true) => format!(
                "Cluster \"{name}\" is being updated with {summary}. This may take a few minutes."
            ),
            _ => format!("Cluster \"{name}\" is being updated. This may take a few minutes."),
        };
        let mut structured = json!({ "originalTier": current, "targetTier": new_size, "computeAutoScaling": enabled, "resolvedProvider": provider, "resolvedRegion": region, "clusterId": c["id"] });
        if let Some(mn) = min {
            structured["minInstanceSize"] = json!(mn);
        }
        if let Some(mx) = max {
            structured["maxInstanceSize"] = json!(mx);
        }
        return Ok(ok(vec![text(t)], Some(structured)));
    }
    if let Some(t) = target.filter(|t| *t != "FLEX" && *t != "M10") {
        return Err(Failure::Plain(format!(
            "targetTier \"{t}\" is not valid when upgrading from {}. Choose FLEX or M10 - larger instance sizes are only valid once the cluster is already Dedicated.",
            i.instance_type
        )));
    }
    let to = target
        .unwrap_or(if i.instance_type == "FREE" {
            "FLEX"
        } else {
            "M10"
        })
        .to_owned();
    if i.instance_type == "FLEX" && to == "FLEX" {
        return Err(Failure::Plain(format!(
            "Cluster \"{name}\" is already a Flex cluster."
        )));
    }
    if to == "FLEX" && autoscaling_args {
        return Err(Failure::Plain("Invalid Arguments:computeAutoScaling/minInstanceSize/maxInstanceSize. Flex clusters do not support compute autoscaling.".to_owned()));
    }
    let enabled = compute_arg.unwrap_or(true);
    let (min, max) = if to == "M10" && enabled {
        (
            Some(min_arg.unwrap_or("M10").to_owned()),
            Some(max_arg.map_or_else(|| max_autoscaling("M10", &provider), str::to_owned)),
        )
    } else {
        (None, None)
    };
    let ready_at = ctx.now + 1;
    if let Some(doc) = ctx.coll_mut("clusters").get_mut(&key) {
        if to == "FLEX" {
            if let Some(m) = doc.as_object_mut() {
                m.shift_remove("replicationSpecs");
            }
            doc["providerSettings"] = json!({ "providerName": "FLEX", "backingProviderName": provider, "regionName": region });
        } else {
            if let Some(m) = doc.as_object_mut() {
                m.shift_remove("providerSettings");
            }
            doc["replicationSpecs"] = json!([{ "zoneName": "Zone 1", "regionConfigs": [{
                "providerName": provider, "regionName": region, "priority": 7,
                "electableSpecs": { "instanceSize": "M10", "nodeCount": 3 },
                "autoScaling": { "compute": match (&min, &max) { (Some(mn), Some(mx)) => json!({ "enabled": true, "scaleDownEnabled": true, "minInstanceSize": mn, "maxInstanceSize": mx }), _ => json!({ "enabled": false, "scaleDownEnabled": false }) }, "diskGB": { "enabled": true } },
            }]}]);
        }
        doc["stateName"] = json!("UPDATING");
        doc["_readyAt"] = json!(ready_at);
    }
    let t = if autoscaling_args {
        format!(
            "Cluster \"{name}\" is being upgraded from {} to {to} tier with compute autoscaling {}. This may take a few minutes.",
            i.instance_type,
            if enabled {
                format!(
                    "enabled ({}-{})",
                    min.clone().unwrap_or_default(),
                    max.clone().unwrap_or_default()
                )
            } else {
                "disabled".to_owned()
            }
        )
    } else {
        format!(
            "Cluster \"{name}\" is being upgraded from {} to {to} tier. This may take a few minutes.",
            i.instance_type
        )
    };
    Ok(ok(
        vec![text(t)],
        Some(
            json!({ "originalTier": i.instance_type, "targetTier": to, "resolvedProvider": provider, "resolvedRegion": region, "clusterId": c["id"] }),
        ),
    ))
}

fn connect_cluster(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "clusterName").to_owned();
    let conn_type = str_arg(args, "connectionType")
        .unwrap_or("standard")
        .to_owned();
    let ip = ensure_current_ip(ctx, &pid);
    let existing = ctx.docs("connections").into_iter().find(|e| {
        (e["state"] == "connected" || e["state"] == "connecting")
            && e["atlasCluster"]["projectId"] == pid.as_str()
            && e["atlasCluster"]["clusterName"] == name.as_str()
    });
    let created_user = existing.is_none();
    let c = cluster(ctx, &pid, &name)?;
    let i = info(&c);
    let entry = match existing {
        Some(e) => e,
        None => {
            let cs = match conn_type.as_str() {
                "private" => c["connectionStrings"]["privateSrv"]
                    .as_str()
                    .or_else(|| c["connectionStrings"]["private"].as_str()),
                "privateEndpoint" => {
                    c["connectionStrings"]["privateEndpoint"][0]["srvConnectionString"].as_str()
                }
                _ => c["connectionStrings"]["standardSrv"]
                    .as_str()
                    .or_else(|| c["connectionStrings"]["standard"].as_str()),
            };
            if cs.is_none() {
                return Err(Failure::Running(format!(
                    "Connection string for connection type \"{conn_type}\" is not available. Please ensure this connection type is set up in Atlas. See https://www.mongodb.com/docs/atlas/connect-to-database-deployment/#connect-to-an-atlas-cluster."
                )));
            }
            let n = ctx.svc.ids.next_uuid("temp-user");
            let num = u32::from_be_bytes([
                n.as_bytes()[0],
                n.as_bytes()[1],
                n.as_bytes()[2],
                n.as_bytes()[3],
            ]) % 100_000;
            let username = format!("mcpUser{num}");
            let expiry = crate::ejson::iso_millis((ctx.now + 14_400) * 1000);
            let user_key = format!("{pid}/{username}");
            ctx.coll_mut("db_users").insert(
                user_key.clone(),
                json!({
                    "databaseName": "admin", "groupId": pid, "username": username,
                    "roles": [{ "roleName": "readWriteAnyDatabase", "databaseName": "admin" }],
                    "scopes": [{ "type": "CLUSTER", "name": name }],
                    "awsIAMType": "NONE", "ldapAuthType": "NONE", "oidcAuthType": "NONE", "x509Type": "NONE",
                    "deleteAfterDate": expiry,
                    "description": "MDB MCP Temporary user, see https://dochub.mongodb.org/core/mongodb-mcp-server-tools-considerations",
                }),
            );
            let project_name = project(ctx, &pid)
                .ok()
                .and_then(|p| p["name"].as_str().map(str::to_owned));
            let cluster_slug = slugify(&name);
            let slug = match project_name {
                Some(pn) => {
                    let ps: String = slugify(&pn)
                        .chars()
                        .take(40usize.saturating_sub(cluster_slug.len() + 1))
                        .collect();
                    if ps.is_empty() {
                        cluster_slug
                    } else {
                        format!("{ps}-{cluster_slug}")
                    }
                }
                None => cluster_slug,
            };
            let entry_name = format!("{slug}-{}", suffix(ctx));
            let id = ctx.uuid("connection");
            let now = ctx.now_iso();
            let state = if i.paused || c["stateName"] != "IDLE" {
                "connecting"
            } else {
                "connected"
            };
            let e = json!({
                "connectionId": id, "name": entry_name, "source": "explicit", "state": state,
                "createdAt": now, "lastUsedAt": now,
                "atlasCluster": { "projectId": pid, "clusterName": name, "clusterId": c["id"], "instanceType": i.instance_type },
                "temporaryUser": user_key,
            });
            ctx.coll_mut("connections").insert(id, e.clone());
            e
        }
    };
    let id = entry["connectionId"].as_str().unwrap_or("").to_owned();
    let connected = entry["state"] == "connected";
    let mut content = if connected {
        vec![text(format!(
            "Connected to cluster \"{name}\". Your connectionId is \"{id}\" — pass it as the connectionId argument to all MongoDB tool calls that should run against this cluster."
        ))]
    } else {
        vec![
            text(format!(
                "Attempting to connect to cluster \"{name}\". Your connectionId is \"{id}\" — pass it as the connectionId argument to MongoDB tool calls once the connection is established."
            )),
            text(
                "Warning: Provisioning a user and connecting to the cluster may take more time, please check again in a few seconds.",
            ),
        ]
    };
    if ip == "added" {
        content.push(text(ACCESS_LIST_ADDED_NOTE));
    }
    if created_user {
        content.push(text(CREATED_USER_MESSAGE));
    }
    let mut structured = json!({ "connectionId": id, "state": if connected { "connected" } else { "connecting" }, "addedCurrentIp": ip == "added", "createdTemporaryUser": created_user });
    if created_user {
        structured["temporaryUserClarification"] = json!(CREATED_USER_MESSAGE);
    }
    if i.instance_type == "FREE" || i.instance_type == "FLEX" {
        let tier = if i.instance_type == "FREE" {
            "Free"
        } else {
            "Flex"
        };
        let mut metrics: Vec<String> = ctx
            .docs("alerts")
            .iter()
            .filter(|a| {
                a["groupId"] == pid.as_str()
                    && a["status"] == "OPEN"
                    && a["clusterName"] == name.as_str()
            })
            .filter(|a| {
                matches!(
                    a["eventTypeName"].as_str(),
                    Some("OUTSIDE_METRIC_THRESHOLD" | "OUTSIDE_FLEX_METRIC_THRESHOLD")
                )
            })
            .filter_map(|a| a["metricName"].as_str().map(str::to_owned))
            .collect();
        metrics.sort();
        metrics.dedup();
        if metrics.is_empty() {
            structured["sharedTierAlertsDetected"] = json!(false);
        } else {
            content.push(text(format!(
                "Note: Atlas reports open shared-tier threshold alerts for cluster \"{name}\" affecting: {}. You may be near connection or storage limits on this {tier} tier deployment. Consider upgrading to a paid tier for more headroom — use the atlas-upgrade-cluster tool to upgrade \"{name}\".",
                metrics.join(", ")
            )));
            structured["sharedTierAlertsDetected"] = json!(true);
            structured["sharedTierAlerts"] = json!(metrics);
        }
        structured["sharedTierTier"] = json!(tier);
    }
    Ok(ok(content, Some(structured)))
}

fn create_access_list(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let ips: Vec<String> = args["ipAddresses"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let cidrs: Vec<String> = args["cidrBlocks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let current = args
        .get("currentIpAddress")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if ips.is_empty() && cidrs.is_empty() && !current {
        return Err(Failure::Running(
            "One of ipAddresses, cidrBlocks, currentIpAddress must be provided.".to_owned(),
        ));
    }
    project(ctx, &pid)?;
    let comment = str_arg(args, "comment")
        .filter(|c| !c.is_empty())
        .unwrap_or(DEFAULT_ACCESS_LIST_COMMENT)
        .to_owned();
    let mut all_ips = ips;
    if current {
        all_ips.push(client_ip(ctx));
    }
    for ip in all_ips {
        ctx.coll_mut("access_list").insert(
            format!("{pid}/{ip}/32"),
            json!({ "groupId": pid, "ipAddress": ip, "cidrBlock": format!("{ip}/32"), "comment": comment }),
        );
    }
    for cidr in cidrs {
        ctx.coll_mut("access_list").insert(
            format!("{pid}/{cidr}"),
            json!({ "groupId": pid, "cidrBlock": cidr, "comment": comment }),
        );
    }
    Ok(ok(
        vec![text(format!(
            "IP/CIDR ranges added to access list for project {pid}."
        ))],
        Some(json!({ "projectId": pid })),
    ))
}

fn inspect_access_list(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    project(ctx, &pid)?;
    let entries: Vec<Value> = ctx
        .docs("access_list")
        .iter()
        .filter(|e| e["groupId"] == pid.as_str())
        .map(|e| {
            let mut m = Map::new();
            insert_some(&mut m, "ipAddress", e.get("ipAddress").cloned());
            insert_some(&mut m, "cidrBlock", e.get("cidrBlock").cloned());
            insert_some(&mut m, "comment", e.get("comment").cloned());
            Value::Object(m)
        })
        .collect();
    if entries.is_empty() {
        return Ok(ok(
            vec![text("No access list entries found.")],
            Some(json!({ "projectId": pid, "entries": [], "totalCount": 0 })),
        ));
    }
    let content = ctx.untrusted(
        &format!("Found {} access list entries", entries.len()),
        vec![Value::Array(entries.clone()).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "projectId": pid, "entries": entries, "totalCount": entries.len() })),
    ))
}

fn list_db_users(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    project(ctx, &pid)?;
    let users: Vec<Value> = ctx
        .docs("db_users")
        .iter()
        .filter(|u| u["groupId"] == pid.as_str())
        .map(|u| {
            let roles: Vec<Value> = u["roles"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| {
                    let mut m = Map::new();
                    m.insert("roleName".to_owned(), r["roleName"].clone());
                    m.insert("databaseName".to_owned(), r["databaseName"].clone());
                    insert_some(&mut m, "collectionName", r.get("collectionName").cloned());
                    Value::Object(m)
                })
                .collect();
            let scopes: Vec<Value> = u["scopes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|sc| json!({ "type": sc["type"], "name": sc["name"] }))
                .collect();
            json!({ "username": u["username"], "roles": roles, "scopes": scopes })
        })
        .collect();
    if users.is_empty() {
        return Ok(ok(
            vec![text(" No database users found")],
            Some(json!({ "projectId": pid, "users": [], "totalCount": 0 })),
        ));
    }
    let content = ctx.untrusted(
        &format!("Found {} database users in project {pid}", users.len()),
        vec![Value::Array(users.clone()).to_string()],
    );
    Ok(ok(
        content,
        Some(json!({ "projectId": pid, "users": users, "totalCount": users.len() })),
    ))
}

fn create_db_user(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let username = s(args, "username").to_owned();
    project(ctx, &pid)?;
    let ip = ensure_current_ip(ctx, &pid);
    let key = format!("{pid}/{username}");
    if ctx.coll("db_users").is_some_and(|c| c.contains(&key)) {
        return Err(ApiError::conflict(format!(
            "The specified database user {username} already exists in group {pid}."
        ))
        .into());
    }
    let generated = str_arg(args, "password").is_none();
    let password = match str_arg(args, "password") {
        Some(p) => p.to_owned(),
        None => ctx.svc.ids.next_prefixed("password", "", 22),
    };
    let roles: Vec<Value> = args["roles"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let mut m = Map::new();
            m.insert("roleName".to_owned(), r["roleName"].clone());
            m.insert(
                "databaseName".to_owned(),
                r.get("databaseName").cloned().unwrap_or(json!("admin")),
            );
            insert_some(&mut m, "collectionName", r.get("collectionName").cloned());
            Value::Object(m)
        })
        .collect();
    let scopes: Vec<Value> = args["clusters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| json!({ "type": "CLUSTER", "name": c }))
        .collect();
    for sc in &scopes {
        let cname = sc["name"].as_str().unwrap_or("");
        if !ctx
            .coll("clusters")
            .is_some_and(|c| c.contains(&cluster_key(&pid, cname)))
        {
            return Err(ApiError::bad_request(format!(
                "Cluster {cname} does not exist in group {pid}."
            ))
            .into());
        }
    }
    ctx.coll_mut("db_users").insert(
        key,
        json!({
            "databaseName": "admin", "groupId": pid, "username": username, "roles": roles, "scopes": scopes,
            "awsIAMType": "NONE", "ldapAuthType": "NONE", "oidcAuthType": "NONE", "x509Type": "NONE",
        }),
    );
    let mut content = vec![text(format!(
        "User \"{username}\" created successfully{}.",
        if generated {
            format!(" with password: `{password}`")
        } else {
            String::new()
        }
    ))];
    content.extend(note(ip));
    let mut structured = json!({ "username": username });
    if generated {
        structured["password"] = json!(password);
    }
    Ok(ok(content, Some(structured)))
}

fn list_alerts(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    project(ctx, &pid)?;
    let status = str_arg(args, "status").unwrap_or("OPEN").to_owned();
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let include_count = args
        .get("includeCount")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let all: Vec<Value> = ctx
        .docs("alerts")
        .into_iter()
        .filter(|a| a["groupId"] == pid.as_str() && a["status"] == status.as_str())
        .collect();
    let (items, total) = page(all, args);
    if items.is_empty() {
        let mut structured = json!({ "projectId": pid, "status": status, "alerts": [] });
        if include_count {
            structured["totalCount"] = json!(total);
        }
        return Ok(ok(
            vec![text(format!(
                "No alerts with status \"{status}\" found in your MongoDB Atlas project."
            ))],
            Some(structured),
        ));
    }
    let iso = |v: &Value| {
        v.as_str()
            .and_then(crate::ejson::parse_iso_millis)
            .map_or("N/A".to_owned(), crate::ejson::iso_millis)
    };
    let alerts: Vec<Value> = items
        .iter()
        .map(|a| json!({
            "id": a["id"],
            "status": a["status"],
            "created": iso(&a["created"]),
            "updated": iso(&a["updated"]),
            "eventTypeName": a["eventTypeName"],
            "acknowledgementComment": a.get("acknowledgementComment").cloned().filter(|v| !v.is_null()).unwrap_or(json!("N/A")),
        }))
        .collect();
    let accurate = include_count && total > 0;
    let content = ctx.untrusted(
        &format!(
            "Found {} alerts with status \"{status}\" in project {pid}{}{}",
            alerts.len(),
            if accurate {
                format!(" (total: {total})")
            } else {
                String::new()
            },
            if alerts.len() == limit {
                ". Use pagination arguments if more results are expected."
            } else {
                ""
            }
        ),
        vec![Value::Array(alerts.clone()).to_string()],
    );
    let mut structured = json!({ "projectId": pid, "status": status, "alerts": alerts });
    if accurate {
        structured["totalCount"] = json!(total);
    }
    Ok(ok(content, Some(structured)))
}

fn performance_advisor(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let name = s(args, "clusterName").to_owned();
    let ops: Vec<String> = args["operations"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| {
            [
                "suggestedIndexes",
                "dropIndexSuggestions",
                "slowQueryLogs",
                "schemaSuggestions",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
        });
    let has = |o: &str| ops.iter().any(|x| x == o);
    // Every sub-request fails when the cluster or project doesn't exist; the tool then
    // reports no recommendations (Promise.allSettled swallows the rejections).
    let found = cluster(ctx, &pid, &name).is_ok();
    let pa = ctx
        .coll("performance_advisor")
        .and_then(|c| c.get(&cluster_key(&pid, &name)))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arr = |k: &str| {
        if found {
            pa[k].as_array().cloned().unwrap_or_default()
        } else {
            vec![]
        }
    };
    let suggested = arr("suggestedIndexes");
    let drop = if found {
        pa["dropIndexSuggestions"].clone()
    } else {
        Value::Null
    };
    let drop = json!({
        "hiddenIndexes": drop["hiddenIndexes"].as_array().cloned().unwrap_or_default(),
        "redundantIndexes": drop["redundantIndexes"].as_array().cloned().unwrap_or_default(),
        "unusedIndexes": drop["unusedIndexes"].as_array().cloned().unwrap_or_default(),
    });
    let since = str_arg(args, "since").and_then(crate::ejson::parse_iso_millis);
    let namespaces: Option<Vec<String>> = args["namespaces"].as_array().map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    });
    let slow: Vec<Value> = arr("slowQueryLogs")
        .into_iter()
        .filter(|l| {
            namespaces.as_ref().is_none_or(|ns| {
                l["namespace"]
                    .as_str()
                    .is_some_and(|n| ns.iter().any(|x| x == n))
            })
        })
        .filter(|l| since.is_none_or(|s| l["_ts"].as_i64().is_none_or(|t| t >= s)))
        .take(50)
        .collect();
    let schema = arr("schemaSuggestions");
    let has_drop = ["hiddenIndexes", "redundantIndexes", "unusedIndexes"]
        .iter()
        .any(|k| drop[*k].as_array().is_some_and(|a| !a.is_empty()));
    let mut structured = json!({ "projectId": pid, "clusterName": name });
    if has("suggestedIndexes") {
        structured["suggestedIndexes"] = json!(suggested);
    }
    if has("dropIndexSuggestions") {
        structured["dropIndexSuggestions"] = drop.clone();
    }
    if has("slowQueryLogs") {
        structured["slowQueryLogs"] = json!(slow);
    }
    if has("schemaSuggestions") {
        structured["schemaSuggestions"] = json!(schema);
    }
    let any = (has("suggestedIndexes") && !suggested.is_empty())
        || (has("dropIndexSuggestions") && has_drop)
        || (has("slowQueryLogs") && !slow.is_empty())
        || (has("schemaSuggestions") && !schema.is_empty());
    if !any {
        return Ok(ok(
            vec![text("No performance advisor recommendations found.")],
            Some(structured),
        ));
    }
    let sections = [
        format!(
            "## Suggested Indexes\n{}",
            if has("suggestedIndexes") && !suggested.is_empty() {
                format!(
                    "{SUGGESTED_INDEXES_COPY}\n{}",
                    Value::Array(suggested.clone())
                )
            } else {
                "No suggested indexes found.".to_owned()
            }
        ),
        format!(
            "## Drop Index Suggestions\n{}",
            if has("dropIndexSuggestions") && has_drop {
                drop.to_string()
            } else {
                "No drop index suggestions found.".to_owned()
            }
        ),
        format!(
            "## Slow Query Logs\n{}",
            if has("slowQueryLogs") && !slow.is_empty() {
                format!("{SLOW_QUERY_LOGS_COPY}\n{}", Value::Array(slow.clone()))
            } else {
                "No slow query logs found.".to_owned()
            }
        ),
        format!(
            "## Schema Suggestions\n{}",
            if has("schemaSuggestions") && !schema.is_empty() {
                Value::Array(schema.clone()).to_string()
            } else {
                "No schema suggestions found.".to_owned()
            }
        ),
    ];
    let content = ctx.untrusted("Performance advisor data", vec![sections.join("\n\n")]);
    Ok(ok(content, Some(structured)))
}

fn get_regions(args: &Value) -> R {
    let provider = s(args, "provider").to_owned();
    let all: Value = serde_json::from_str(REGIONS).expect("regions asset is JSON");
    let Some(regions) = all.get(&provider).cloned() else {
        return Err(Failure::Plain("MCP error -32602: Input validation error: Invalid arguments for tool atlas-get-regions: [\n  {\n    \"code\": \"invalid_value\",\n    \"values\": [\n      \"AWS\",\n      \"GCP\",\n      \"AZURE\"\n    ],\n    \"path\": [\n      \"provider\"\n    ],\n    \"message\": \"Invalid option: expected one of \\\"AWS\\\"|\\\"GCP\\\"|\\\"AZURE\\\"\"\n  }\n]".to_string()));
    };
    let structured = json!({ "provider": provider, "regions": regions });
    Ok(ok(
        vec![text(format!(
            "Supported MongoDB Atlas regions for {provider}:\n{}",
            serde_json::to_string_pretty(&structured).expect("serializes")
        ))],
        Some(structured),
    ))
}

fn load_sample_dataset(ctx: &mut Ctx, args: &Value) -> R {
    let pid = s(args, "projectId").to_owned();
    let cluster_name = str_arg(args, "clusterName");
    let job = str_arg(args, "jobId");
    let (status, header) = match (cluster_name, job) {
        (None, Some(job)) => {
            project(ctx, &pid)?;
            let mut st = ctx
                .coll("sample_loads")
                .and_then(|c| c.get(job))
                .cloned()
                .ok_or_else(|| ApiError::not_found(format!("No sample dataset load with ID {job} exists.")))?;
            if st["state"] == "WORKING" {
                st["state"] = json!("COMPLETED");
                st["completeDate"] = json!(ctx.now_iso());
                ctx.coll_mut("sample_loads").insert(job, st.clone());
            }
            let h = format!("Sample dataset load status for cluster \"{}\" in project {pid}.", st["clusterName"].as_str().unwrap_or(""));
            (st, h)
        }
        (Some(name), None) => {
            cluster(ctx, &pid, name)?;
            let id = ctx.object_id("sample-load");
            let st = json!({ "_id": id, "clusterName": name, "state": "WORKING", "createDate": ctx.now_iso() });
            ctx.coll_mut("sample_loads").insert(id.clone(), st.clone());
            let h = format!("Sample dataset load requested for cluster \"{name}\" in project {pid}.");
            (st, h)
        }
        _ => return Err(Failure::Running("Provide exactly one of `clusterName` (to initiate a new sample dataset load) or `jobId` (to check the status of a previous load).".to_owned())),
    };
    let mut structured = json!({ "jobId": status["_id"], "clusterName": status["clusterName"], "state": status["state"], "createDate": status["createDate"] });
    if let Some(d) = status.get("completeDate") {
        structured["completeDate"] = d.clone();
    }
    if let Some(e) = status.get("errorMessage") {
        structured["errorMessage"] = e.clone();
    }
    Ok(ok(
        vec![text(header), text(structured.to_string())],
        Some(structured),
    ))
}
