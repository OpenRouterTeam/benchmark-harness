//! EC2 (ec2 query protocol). Documents use EC2's own XML field names (`instanceId`,
//! `instanceState`, `tagSet`, ...) so they serialize directly; lists become `<item>`.
//! Fields starting with `_` are internal (`_region`, `_reservationId`).
//!
//! Collections: `instances`, `security_groups`, `vpcs`, `subnets`, `volumes`, `images`,
//! `snapshots`, `key_pairs`, `addresses`, each keyed by its id (`key_pairs` by name).

use axum::{http::StatusCode, response::Response};
use serde_json::{Value, json};

use crate::{
    Ctx, Req,
    util::{Params, el, esc, iso, s, tel, to_xml, xml_response},
};

const NS: &str = "http://ec2.amazonaws.com/doc/2016-11-15/";

type Out = Result<String, (StatusCode, String, String)>;

fn err(code: &str, msg: impl Into<String>) -> (StatusCode, String, String) {
    (StatusCode::BAD_REQUEST, code.to_owned(), msg.into())
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let p = Params::parse(&req.body_text());
    let action = p.str("Action");
    let rid = ctx.request_id();
    if p.bool("DryRun") == Some(true) {
        return error_response(
            StatusCode::PRECONDITION_FAILED,
            "DryRunOperation",
            "Request would have succeeded, but DryRun flag is set.",
            &rid,
        );
    }
    match run(ctx, &action, &p) {
        Ok(body) => xml_response(
            StatusCode::OK,
            format!(
                "<{action}Response xmlns=\"{NS}\"><requestId>{rid}</requestId>{body}</{action}Response>"
            ),
        ),
        Err((status, code, msg)) => error_response(status, &code, &msg, &rid),
    }
}

fn error_response(status: StatusCode, code: &str, msg: &str, rid: &str) -> Response {
    xml_response(
        status,
        format!(
            "<Response><Errors><Error><Code>{code}</Code><Message>{}</Message></Error></Errors><RequestID>{rid}</RequestID></Response>",
            esc(msg)
        ),
    )
}

fn run(ctx: &mut Ctx, action: &str, p: &Params) -> Out {
    match action {
        "DescribeInstances" => describe_instances(ctx, p),
        "DescribeInstanceStatus" => describe_instance_status(ctx, p),
        "StartInstances" => transition(ctx, p, "start"),
        "StopInstances" => transition(ctx, p, "stop"),
        "TerminateInstances" => transition(ctx, p, "terminate"),
        "RebootInstances" => {
            instances_by_ids(ctx, &p.list("InstanceId"))?;
            Ok("<return>true</return>".into())
        }
        "RunInstances" => run_instances(ctx, p),
        "ModifyInstanceAttribute" => modify_instance_attribute(ctx, p),
        "DescribeInstanceAttribute" => describe_instance_attribute(ctx, p),
        "CreateTags" => create_tags(ctx, p, true),
        "DeleteTags" => create_tags(ctx, p, false),
        "DescribeTags" => describe_tags(ctx, p),
        "DescribeSecurityGroups" => describe_generic(
            ctx,
            p,
            "security_groups",
            "groupId",
            "GroupId",
            "securityGroupInfo",
            "InvalidGroup.NotFound",
            "The security group '{}' does not exist",
        ),
        "DescribeVpcs" => describe_generic(
            ctx,
            p,
            "vpcs",
            "vpcId",
            "VpcId",
            "vpcSet",
            "InvalidVpcID.NotFound",
            "The vpc ID '{}' does not exist",
        ),
        "DescribeSubnets" => describe_generic(
            ctx,
            p,
            "subnets",
            "subnetId",
            "SubnetId",
            "subnetSet",
            "InvalidSubnetID.NotFound",
            "The subnet ID '{}' does not exist",
        ),
        "DescribeVolumes" => describe_generic(
            ctx,
            p,
            "volumes",
            "volumeId",
            "VolumeId",
            "volumeSet",
            "InvalidVolume.NotFound",
            "The volume '{}' does not exist.",
        ),
        "DescribeImages" => describe_generic(
            ctx,
            p,
            "images",
            "imageId",
            "ImageId",
            "imagesSet",
            "InvalidAMIID.NotFound",
            "The image id '[{}]' does not exist",
        ),
        "DescribeSnapshots" => describe_generic(
            ctx,
            p,
            "snapshots",
            "snapshotId",
            "SnapshotId",
            "snapshotSet",
            "InvalidSnapshot.NotFound",
            "The snapshot '{}' does not exist.",
        ),
        "DescribeKeyPairs" => describe_generic(
            ctx,
            p,
            "key_pairs",
            "keyName",
            "KeyName",
            "keySet",
            "InvalidKeyPair.NotFound",
            "The key pair '{}' does not exist",
        ),
        "DescribeAddresses" => describe_generic(
            ctx,
            p,
            "addresses",
            "allocationId",
            "AllocationId",
            "addressesSet",
            "InvalidAllocationID.NotFound",
            "The allocation ID '{}' does not exist",
        ),
        "CreateSecurityGroup" => create_security_group(ctx, p),
        "DeleteSecurityGroup" => delete_security_group(ctx, p),
        "AuthorizeSecurityGroupIngress" => sg_rules(ctx, p, "ipPermissions", true),
        "AuthorizeSecurityGroupEgress" => sg_rules(ctx, p, "ipPermissionsEgress", true),
        "RevokeSecurityGroupIngress" => sg_rules(ctx, p, "ipPermissions", false),
        "RevokeSecurityGroupEgress" => sg_rules(ctx, p, "ipPermissionsEgress", false),
        "DescribeRegions" => Ok(describe_regions(p)),
        "DescribeAvailabilityZones" => Ok(describe_azs(ctx)),
        "DescribeAccountAttributes" => Ok(format!(
            "<accountAttributeSet><item><attributeName>supported-platforms</attributeName><attributeValueSet><item><attributeValue>VPC</attributeValue></item></attributeValueSet></item><item><attributeName>default-vpc</attributeName><attributeValueSet><item><attributeValue>{}</attributeValue></item></attributeValueSet></item></accountAttributeSet>",
            ctx.docs("vpcs")
                .iter()
                .find(|(_, v)| v["isDefault"] == json!(true))
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "none".into())
        )),
        "" => Err(err(
            "MissingAction",
            "The request must contain the parameter Action",
        )),
        a if a.starts_with("Describe") || a.starts_with("Get") || a.starts_with("List") => {
            Ok(String::new())
        }
        other => Err(err(
            "InvalidAction",
            format!("The action {other} is not valid for this web service."),
        )),
    }
}

// ───────────────────────────── filters ─────────────────────────────

struct Filter {
    name: String,
    values: Vec<String>,
}

fn filters(p: &Params) -> Vec<Filter> {
    p.indexes("Filter")
        .into_iter()
        .map(|n| Filter {
            name: p.str(&format!("Filter.{n}.Name")),
            values: p.list(&format!("Filter.{n}.Value")),
        })
        .collect()
}

fn tag_value(doc: &Value, key: &str) -> Option<String> {
    doc.get("tagSet")?
        .as_array()?
        .iter()
        .find(|t| t["key"] == key)
        .map(|t| s(t, "value"))
}

/// EC2 filter names (`instance-state-name`, `tag:Name`, `vpc-id`, ...) to document values.
fn field_values(doc: &Value, name: &str) -> Vec<String> {
    if let Some(key) = name.strip_prefix("tag:") {
        return tag_value(doc, key).into_iter().collect();
    }
    let path: &[&str] = match name {
        "tag-key" => {
            return doc["tagSet"]
                .as_array()
                .map(|a| a.iter().map(|t| s(t, "key")).collect())
                .unwrap_or_default();
        }
        "tag-value" => {
            return doc["tagSet"]
                .as_array()
                .map(|a| a.iter().map(|t| s(t, "value")).collect())
                .unwrap_or_default();
        }
        "instance-state-name" => &["instanceState", "name"],
        "instance-state-code" => &["instanceState", "code"],
        "availability-zone" if doc.get("placement").is_some() => &["placement", "availabilityZone"],
        "availability-zone" => &["availabilityZone"],
        "instance.group-id" | "group-id" if doc.get("groupSet").is_some() => {
            return doc["groupSet"]
                .as_array()
                .map(|a| a.iter().map(|g| s(g, "groupId")).collect())
                .unwrap_or_default();
        }
        "instance.group-name" | "group-name" if doc.get("groupSet").is_some() => {
            return doc["groupSet"]
                .as_array()
                .map(|a| a.iter().map(|g| s(g, "groupName")).collect())
                .unwrap_or_default();
        }
        "attachment.instance-id" => {
            return doc["attachmentSet"]
                .as_array()
                .map(|a| a.iter().map(|g| s(g, "instanceId")).collect())
                .unwrap_or_default();
        }
        "owner-id" => &["ownerId"],
        "state" if doc.get("instanceState").is_none() => &["state"],
        "is-default" => &["isDefault"],
        "ip-permission.from-port" => {
            return doc["ipPermissions"]
                .as_array()
                .map(|a| a.iter().map(|r| r["fromPort"].to_string()).collect())
                .unwrap_or_default();
        }
        "ip-permission.cidr" => {
            return doc["ipPermissions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .flat_map(|r| r["ipRanges"].as_array().cloned().unwrap_or_default())
                        .map(|r| s(&r, "cidrIp"))
                        .collect()
                })
                .unwrap_or_default();
        }
        other => {
            // `instance-type` -> `instanceType`, `vpc-id` -> `vpcId`, ...
            let camel = camel(other);
            return match doc.get(&camel) {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Number(n)) => vec![n.to_string()],
                Some(Value::Bool(b)) => vec![b.to_string()],
                _ => Vec::new(),
            };
        }
    };
    let mut v = doc;
    for seg in path {
        match v.get(seg) {
            Some(next) => v = next,
            None => return Vec::new(),
        }
    }
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Number(n) => vec![n.to_string()],
        Value::Bool(b) => vec![b.to_string()],
        _ => Vec::new(),
    }
}

fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '-' || c == '.' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn passes(doc: &Value, fs: &[Filter]) -> bool {
    fs.iter().all(|f| {
        let have = field_values(doc, &f.name);
        f.values.iter().any(|want| {
            have.iter().any(|h| {
                crate::util::glob_match(want, h)
                    && (want.contains('*') || want.contains('?') || h == want)
            })
        })
    })
}

// ───────────────────────────── instances ─────────────────────────────

fn regional(ctx: &Ctx, collection: &str) -> Vec<(String, Value)> {
    ctx.docs(collection)
        .into_iter()
        .filter(|(_, d)| ctx.in_region(d))
        .collect()
}

fn instances_by_ids(ctx: &Ctx, ids: &[String]) -> Result<Vec<Value>, (StatusCode, String, String)> {
    let all = regional(ctx, "instances");
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for id in ids {
        if !id.starts_with("i-") {
            return Err(err(
                "InvalidInstanceID.Malformed",
                format!("Invalid id: \"{id}\""),
            ));
        }
        match all.iter().find(|(k, _)| k == id) {
            Some((_, d)) => out.push(d.clone()),
            None => missing.push(id.clone()),
        }
    }
    match missing.len() {
        0 => Ok(out),
        1 => Err(err(
            "InvalidInstanceID.NotFound",
            format!("The instance ID '{}' does not exist", missing[0]),
        )),
        _ => Err(err(
            "InvalidInstanceID.NotFound",
            format!("The instance IDs '{}' do not exist", missing.join(", ")),
        )),
    }
}

fn reservation_id(doc: &Value) -> String {
    doc.get("_reservationId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("r-{}", s(doc, "instanceId").trim_start_matches("i-")))
}

fn describe_instances(ctx: &mut Ctx, p: &Params) -> Out {
    let ids = p.list("InstanceId");
    let docs = if ids.is_empty() {
        regional(ctx, "instances")
            .into_iter()
            .map(|(_, d)| d)
            .collect()
    } else {
        instances_by_ids(ctx, &ids)?
    };
    let fs = filters(p);
    let matched: Vec<Value> = docs.into_iter().filter(|d| passes(d, &fs)).collect();
    let max = p.int("MaxResults").unwrap_or(1000).max(5) as usize;
    let start = p
        .get("NextToken")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let page: Vec<&Value> = matched.iter().skip(start).take(max).collect();
    // Group into reservations, keeping first-seen order.
    let mut reservations: Vec<(String, Vec<&Value>)> = Vec::new();
    for d in page {
        let r = reservation_id(d);
        match reservations.iter_mut().find(|(id, _)| *id == r) {
            Some((_, list)) => list.push(d),
            None => reservations.push((r, vec![d])),
        }
    }
    let owner = ctx.account.clone();
    let mut x = String::from("<reservationSet>");
    for (r, list) in reservations {
        x += &el(
            "item",
            &format!(
                "{}{}<groupSet/>{}",
                tel("reservationId", &r),
                tel("ownerId", &owner),
                el(
                    "instancesSet",
                    &list
                        .iter()
                        .map(|d| el("item", &to_xml(d, "item")))
                        .collect::<String>()
                )
            ),
        );
    }
    x += "</reservationSet>";
    if start + max < matched.len() {
        x += &tel("nextToken", &(start + max).to_string());
    }
    Ok(x)
}

fn describe_instance_status(ctx: &mut Ctx, p: &Params) -> Out {
    let ids = p.list("InstanceId");
    let docs = if ids.is_empty() {
        regional(ctx, "instances")
            .into_iter()
            .map(|(_, d)| d)
            .collect()
    } else {
        instances_by_ids(ctx, &ids)?
    };
    let all = p.bool("IncludeAllInstances").unwrap_or(false);
    let fs = filters(p);
    let items: String = docs
        .iter()
        .filter(|d| all || d["instanceState"]["name"] == "running")
        .filter(|d| passes(d, &fs))
        .map(|d| {
            let running = d["instanceState"]["name"] == "running";
            let st = if running { "ok" } else { "not-applicable" };
            let detail = if running { "passed" } else { "not-applicable" };
            el(
                "item",
                &format!(
                    "{}{}{}<systemStatus><status>{st}</status><details><item><name>reachability</name><status>{detail}</status></item></details></systemStatus><instanceStatus><status>{st}</status><details><item><name>reachability</name><status>{detail}</status></item></details></instanceStatus>",
                    tel("instanceId", &s(d, "instanceId")),
                    tel("availabilityZone", &s(&d["placement"], "availabilityZone")),
                    el("instanceState", &to_xml(&d["instanceState"], "item"))
                ),
            )
        })
        .collect();
    Ok(el("instanceStatusSet", &items))
}

fn state(code: i64, name: &str) -> Value {
    json!({"code": code, "name": name})
}

fn transition(ctx: &mut Ctx, p: &Params, op: &str) -> Out {
    let ids = p.list("InstanceId");
    if ids.is_empty() {
        return Err(err(
            "MissingParameter",
            "The request must contain the parameter InstanceId",
        ));
    }
    let docs = instances_by_ids(ctx, &ids)?;
    let mut items = String::new();
    let stamp = iso(ctx.now).replace('T', " ").replace('Z', " GMT");
    for d in docs {
        let id = s(&d, "instanceId");
        let prev = d["instanceState"].clone();
        let prev_name = s(&prev, "name");
        let (shown, stored) = match (op, prev_name.as_str()) {
            ("stop", "running" | "pending") => (state(64, "stopping"), Some(state(80, "stopped"))),
            ("stop", "stopped" | "stopping") => (prev.clone(), None),
            ("stop", "terminated" | "shutting-down") => {
                return Err(err(
                    "IncorrectInstanceState",
                    format!("This instance '{id}' is not in a state from which it can be stopped."),
                ));
            }
            ("start", "stopped") => (state(0, "pending"), Some(state(16, "running"))),
            ("start", "running" | "pending") => (prev.clone(), None),
            ("start", _) => {
                return Err(err(
                    "IncorrectInstanceState",
                    format!("The instance '{id}' is not in a state from which it can be started."),
                ));
            }
            ("terminate", "terminated") => (prev.clone(), None),
            ("terminate", _) => (state(32, "shutting-down"), Some(state(48, "terminated"))),
            _ => (prev.clone(), None),
        };
        if let Some(new_state) = stored {
            let doc = ctx.get_mut("instances", &id).expect("instance exists");
            doc["instanceState"] = new_state;
            let reason = match op {
                "start" => String::new(),
                _ => format!("User initiated ({stamp})"),
            };
            doc["reason"] = json!(reason);
            if op != "start"
                && let Some(o) = doc.as_object_mut()
            {
                o.shift_remove("publicIpAddress");
                o.shift_remove("ipAddress");
            }
        }
        items += &el(
            "item",
            &format!(
                "{}{}{}",
                tel("instanceId", &id),
                el("currentState", &to_xml(&shown, "item")),
                el("previousState", &to_xml(&prev, "item"))
            ),
        );
    }
    Ok(el("instancesSet", &items))
}

fn tag_specs(p: &Params, resource_type: &str) -> Vec<Value> {
    let mut tags = Vec::new();
    for n in p.indexes("TagSpecification") {
        if p.str(&format!("TagSpecification.{n}.ResourceType")) != resource_type {
            continue;
        }
        for m in p.indexes(&format!("TagSpecification.{n}.Tag")) {
            tags.push(json!({
                "key": p.str(&format!("TagSpecification.{n}.Tag.{m}.Key")),
                "value": p.str(&format!("TagSpecification.{n}.Tag.{m}.Value")),
            }));
        }
    }
    tags
}

fn run_instances(ctx: &mut Ctx, p: &Params) -> Out {
    let image = p.str("ImageId");
    if image.is_empty() {
        return Err(err(
            "MissingParameter",
            "The request must contain the parameter ImageId",
        ));
    }
    if !ctx.docs("images").iter().any(|(k, _)| *k == image)
        && ctx.get("images", &image).is_none()
        && !ctx.docs("images").is_empty()
    {
        return Err(err(
            "InvalidAMIID.NotFound",
            format!("The image id '[{image}]' does not exist"),
        ));
    }
    let count = p.int("MaxCount").unwrap_or(1).clamp(1, 20);
    let subnet_id = p
        .get("SubnetId")
        .map(str::to_owned)
        .or_else(|| regional(ctx, "subnets").first().map(|(k, _)| k.clone()));
    let subnet = subnet_id.as_ref().and_then(|id| ctx.get("subnets", id));
    let az = subnet
        .as_ref()
        .map(|sn| s(sn, "availabilityZone"))
        .unwrap_or_else(|| format!("{}a", ctx.region));
    let vpc = subnet.as_ref().map(|sn| s(sn, "vpcId")).unwrap_or_default();
    let reservation = format!(
        "r-{}",
        ctx.svc
            .ids
            .next_prefixed("reservation", "", 17)
            .to_ascii_lowercase()
    );
    let mut items = String::new();
    for n in 0..count {
        let id = format!("i-{}", hex_id(ctx, "instance"));
        let ip = format!(
            "10.0.{}.{}",
            200 + n / 200,
            10 + ctx.svc.ids.next_int("instance-ip", 10)
        );
        let doc = json!({
            "instanceId": id,
            "imageId": image,
            "instanceState": state(16, "running"),
            "privateDnsName": format!("ip-{}.ec2.internal", ip.replace('.', "-")),
            "reason": "",
            "keyName": p.get("KeyName"),
            "amiLaunchIndex": n,
            "instanceType": p.get("InstanceType").unwrap_or("m1.small"),
            "launchTime": iso(ctx.now),
            "placement": {"availabilityZone": az, "tenancy": "default"},
            "monitoring": {"state": "disabled"},
            "subnetId": subnet_id,
            "vpcId": vpc,
            "privateIpAddress": ip,
            "groupSet": [],
            "architecture": "x86_64",
            "rootDeviceType": "ebs",
            "rootDeviceName": "/dev/xvda",
            "tagSet": tag_specs(p, "instance"),
            "_reservationId": reservation,
            "_region": ctx.region,
        });
        let initial = {
            let mut d = doc.clone();
            d["instanceState"] = state(0, "pending");
            d
        };
        ctx.put("instances", &id, doc);
        items += &el("item", &to_xml(&initial, "item"));
    }
    Ok(format!(
        "{}{}<groupSet/>{}",
        tel("reservationId", &reservation),
        tel("ownerId", &ctx.account),
        el("instancesSet", &items)
    ))
}

fn hex_id(ctx: &mut Ctx, kind: &str) -> String {
    let raw = ctx.svc.ids.next_uuid(kind).simple().to_string();
    raw[..17].to_owned()
}

fn modify_instance_attribute(ctx: &mut Ctx, p: &Params) -> Out {
    let id = p.str("InstanceId");
    let doc = instances_by_ids(ctx, std::slice::from_ref(&id))?.remove(0);
    let new_type = p.get("InstanceType.Value").or_else(|| {
        (p.get("Attribute") == Some("instanceType"))
            .then(|| p.get("Value"))
            .flatten()
    });
    if let Some(t) = new_type {
        if doc["instanceState"]["name"] != "stopped" {
            return Err(err(
                "IncorrectInstanceState",
                format!("The instance '{id}' is not in the 'stopped' state."),
            ));
        }
        ctx.get_mut("instances", &id).expect("instance")["instanceType"] = json!(t);
    }
    if let Some(v) = p.get("DisableApiTermination.Value") {
        ctx.get_mut("instances", &id).expect("instance")["_disableApiTermination"] =
            json!(v == "true");
    }
    Ok("<return>true</return>".into())
}

fn describe_instance_attribute(ctx: &mut Ctx, p: &Params) -> Out {
    let id = p.str("InstanceId");
    let doc = instances_by_ids(ctx, std::slice::from_ref(&id))?.remove(0);
    let attr = p.str("Attribute");
    let value = match attr.as_str() {
        "instanceType" => el("instanceType", &tel("value", &s(&doc, "instanceType"))),
        "disableApiTermination" => el(
            "disableApiTermination",
            &el(
                "value",
                &doc.get("_disableApiTermination")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .to_string(),
            ),
        ),
        "groupSet" => el("groupSet", &to_xml(&doc["groupSet"], "item")),
        _ => String::new(),
    };
    Ok(format!("{}{value}", tel("instanceId", &id)))
}

// ───────────────────────────── tags ─────────────────────────────

fn collection_for(id: &str) -> Option<&'static str> {
    let prefix = id.split('-').next()?;
    Some(match prefix {
        "i" => "instances",
        "sg" => "security_groups",
        "vpc" => "vpcs",
        "subnet" => "subnets",
        "vol" => "volumes",
        "ami" => "images",
        "snap" => "snapshots",
        "eipalloc" => "addresses",
        _ => return None,
    })
}

fn resource_type(collection: &str) -> &'static str {
    match collection {
        "instances" => "instance",
        "security_groups" => "security-group",
        "vpcs" => "vpc",
        "subnets" => "subnet",
        "volumes" => "volume",
        "images" => "image",
        "snapshots" => "snapshot",
        _ => "elastic-ip",
    }
}

fn create_tags(ctx: &mut Ctx, p: &Params, add: bool) -> Out {
    let ids = p.list("ResourceId");
    let tags: Vec<(String, Option<String>)> = p
        .indexes("Tag")
        .into_iter()
        .map(|n| {
            (
                p.str(&format!("Tag.{n}.Key")),
                p.get(&format!("Tag.{n}.Value")).map(str::to_owned),
            )
        })
        .collect();
    for id in &ids {
        let Some(collection) = collection_for(id) else {
            return Err(err("InvalidID", format!("The ID '{id}' is not valid")));
        };
        if ctx.get(collection, id).is_none() {
            let code = match collection {
                "instances" => "InvalidInstanceID.NotFound",
                "security_groups" => "InvalidGroup.NotFound",
                "volumes" => "InvalidVolume.NotFound",
                _ => "InvalidID",
            };
            return Err(err(code, format!("The ID '{id}' does not exist")));
        }
    }
    for id in &ids {
        let collection = collection_for(id).expect("checked");
        let doc = ctx.get_mut(collection, id).expect("checked");
        let mut set: Vec<Value> = doc
            .get("tagSet")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (k, v) in &tags {
            if add {
                let v = v.clone().unwrap_or_default();
                match set.iter_mut().find(|t| t["key"] == k.as_str()) {
                    Some(t) => t["value"] = json!(v),
                    None => set.push(json!({"key": k, "value": v})),
                }
            } else {
                set.retain(|t| {
                    t["key"] != k.as_str() || v.as_ref().is_some_and(|v| t["value"] != v.as_str())
                });
            }
        }
        doc["tagSet"] = Value::Array(set);
    }
    Ok("<return>true</return>".into())
}

fn describe_tags(ctx: &mut Ctx, p: &Params) -> Out {
    let fs = filters(p);
    let mut items = String::new();
    for collection in [
        "instances",
        "security_groups",
        "vpcs",
        "subnets",
        "volumes",
        "images",
        "snapshots",
    ] {
        for (id, doc) in regional(ctx, collection) {
            for t in doc["tagSet"].as_array().cloned().unwrap_or_default() {
                let row = json!({
                    "resourceId": id,
                    "resourceType": resource_type(collection),
                    "key": t["key"],
                    "value": t["value"],
                });
                let ok = fs.iter().all(|f| {
                    let have = match f.name.as_str() {
                        "resource-id" => s(&row, "resourceId"),
                        "resource-type" => s(&row, "resourceType"),
                        "key" => s(&row, "key"),
                        "value" => s(&row, "value"),
                        _ => String::new(),
                    };
                    f.values.iter().any(|v| crate::util::glob_match(v, &have))
                });
                if ok {
                    items += &el("item", &to_xml(&row, "item"));
                }
            }
        }
    }
    Ok(el("tagSet", &items))
}

// ───────────────────────────── other resources ─────────────────────────────

#[allow(clippy::too_many_arguments)]
fn describe_generic(
    ctx: &mut Ctx,
    p: &Params,
    collection: &str,
    id_field: &str,
    id_param: &str,
    set_name: &str,
    not_found_code: &str,
    not_found_msg: &str,
) -> Out {
    let mut ids = p.list(id_param);
    if collection == "security_groups" {
        ids.extend(p.list("GroupName").iter().filter_map(|name| {
            regional(ctx, collection)
                .into_iter()
                .find(|(_, d)| s(d, "groupName") == *name)
                .map(|(k, _)| k)
                .or_else(|| Some(name.clone()))
        }));
    }
    let docs = regional(ctx, collection);
    for id in &ids {
        if !docs.iter().any(|(k, d)| k == id || s(d, id_field) == *id) {
            return Err(err(not_found_code, not_found_msg.replace("{}", id)));
        }
    }
    let fs = filters(p);
    let owners = p.list("Owner");
    let items: String = docs
        .iter()
        .filter(|(k, d)| ids.is_empty() || ids.iter().any(|i| i == k || s(d, id_field) == *i))
        .filter(|(_, d)| passes(d, &fs))
        .filter(|(_, d)| {
            owners.is_empty()
                || owners.iter().any(|o| {
                    (o == "self" && d.get("ownerId").is_none_or(|v| v == ctx.account.as_str()))
                        || d.get("ownerId").is_some_and(|v| v == o.as_str())
                        || d.get("imageOwnerAlias").is_some_and(|v| v == o.as_str())
                })
        })
        .map(|(_, d)| el("item", &to_xml(d, "item")))
        .collect();
    Ok(el(set_name, &items))
}

fn create_security_group(ctx: &mut Ctx, p: &Params) -> Out {
    let name = p.str("GroupName");
    let desc = p.str("GroupDescription");
    if name.is_empty() || desc.is_empty() {
        return Err(err(
            "MissingParameter",
            "The request must contain the parameter GroupName and GroupDescription",
        ));
    }
    let vpc = p.get("VpcId").map(str::to_owned).or_else(|| {
        regional(ctx, "vpcs")
            .into_iter()
            .find(|(_, v)| v["isDefault"] == json!(true))
            .map(|(k, _)| k)
    });
    if regional(ctx, "security_groups").iter().any(|(_, g)| {
        s(g, "groupName") == name && g.get("vpcId").and_then(Value::as_str) == vpc.as_deref()
    }) {
        return Err(err(
            "InvalidGroup.Duplicate",
            format!(
                "The security group '{name}' already exists for VPC '{}'",
                vpc.clone().unwrap_or_default()
            ),
        ));
    }
    let id = format!("sg-{}", hex_id(ctx, "security-group"));
    let doc = json!({
        "ownerId": ctx.account,
        "groupId": id,
        "groupName": name,
        "groupDescription": desc,
        "vpcId": vpc,
        "ipPermissions": [],
        "ipPermissionsEgress": [{"ipProtocol": "-1", "groups": [], "ipRanges": [{"cidrIp": "0.0.0.0/0"}], "ipv6Ranges": [], "prefixListIds": []}],
        "tagSet": tag_specs(p, "security-group"),
        "_region": ctx.region,
    });
    ctx.put("security_groups", &id, doc);
    Ok(format!("<return>true</return>{}", tel("groupId", &id)))
}

fn delete_security_group(ctx: &mut Ctx, p: &Params) -> Out {
    let id = p.get("GroupId").map(str::to_owned).or_else(|| {
        let name = p.str("GroupName");
        regional(ctx, "security_groups")
            .into_iter()
            .find(|(_, g)| s(g, "groupName") == name)
            .map(|(k, _)| k)
    });
    let Some(id) = id.filter(|i| ctx.get("security_groups", i).is_some()) else {
        return Err(err(
            "InvalidGroup.NotFound",
            format!(
                "The security group '{}' does not exist",
                p.get("GroupId").or(p.get("GroupName")).unwrap_or_default()
            ),
        ));
    };
    let in_use = regional(ctx, "instances").iter().any(|(_, i)| {
        i["instanceState"]["name"] != "terminated"
            && i["groupSet"]
                .as_array()
                .is_some_and(|g| g.iter().any(|g| g["groupId"] == id.as_str()))
    });
    if in_use {
        return Err(err(
            "DependencyViolation",
            format!("resource {id} has a dependent object"),
        ));
    }
    ctx.remove("security_groups", &id);
    Ok(format!("<return>true</return>{}", tel("groupId", &id)))
}

fn permissions_param(p: &Params) -> Vec<Value> {
    let mut perms: Vec<Value> = p
        .indexes("IpPermissions")
        .into_iter()
        .map(|n| {
            let base = format!("IpPermissions.{n}");
            let ranges: Vec<Value> = p
                .indexes(&format!("{base}.IpRanges"))
                .into_iter()
                .map(|m| {
                    let mut r = json!({"cidrIp": p.str(&format!("{base}.IpRanges.{m}.CidrIp"))});
                    if let Some(d) = p.get(&format!("{base}.IpRanges.{m}.Description")) {
                        r["description"] = json!(d);
                    }
                    r
                })
                .collect();
            let groups: Vec<Value> = p
                .indexes(&format!("{base}.Groups"))
                .into_iter()
                .map(|m| json!({"groupId": p.str(&format!("{base}.Groups.{m}.GroupId")), "userId": ""}))
                .collect();
            let mut perm = json!({
                "ipProtocol": p.get(&format!("{base}.IpProtocol")).unwrap_or("-1"),
                "groups": groups,
                "ipRanges": ranges,
                "ipv6Ranges": [],
                "prefixListIds": [],
            });
            if let Some(f) = p.int(&format!("{base}.FromPort")) {
                perm["fromPort"] = json!(f);
            }
            if let Some(t) = p.int(&format!("{base}.ToPort")) {
                perm["toPort"] = json!(t);
            }
            perm
        })
        .collect();
    if perms.is_empty() && p.get("CidrIp").is_some() {
        let mut perm = json!({
            "ipProtocol": p.get("IpProtocol").unwrap_or("-1"),
            "groups": [], "ipRanges": [{"cidrIp": p.str("CidrIp")}], "ipv6Ranges": [], "prefixListIds": [],
        });
        if let Some(f) = p.int("FromPort") {
            perm["fromPort"] = json!(f);
        }
        if let Some(t) = p.int("ToPort") {
            perm["toPort"] = json!(t);
        }
        perms.push(perm);
    }
    perms
}

fn same_rule(a: &Value, b: &Value) -> bool {
    let proto = |v: &Value| {
        let p = s(v, "ipProtocol");
        match p.as_str() {
            "6" => "tcp".to_owned(),
            "17" => "udp".to_owned(),
            _ => p,
        }
    };
    proto(a) == proto(b)
        && a.get("fromPort") == b.get("fromPort")
        && a.get("toPort") == b.get("toPort")
}

fn sg_rules(ctx: &mut Ctx, p: &Params, field: &str, add: bool) -> Out {
    let id = p.get("GroupId").map(str::to_owned).or_else(|| {
        let name = p.str("GroupName");
        regional(ctx, "security_groups")
            .into_iter()
            .find(|(_, g)| s(g, "groupName") == name)
            .map(|(k, _)| k)
    });
    let Some(id) = id.filter(|i| ctx.get("security_groups", i).is_some()) else {
        return Err(err(
            "InvalidGroup.NotFound",
            format!(
                "The security group '{}' does not exist",
                p.get("GroupId").or(p.get("GroupName")).unwrap_or_default()
            ),
        ));
    };
    let perms = permissions_param(p);
    let mut group = ctx.get("security_groups", &id).expect("checked");
    let mut existing: Vec<Value> = group[field].as_array().cloned().unwrap_or_default();
    let mut rule_items = String::new();
    for perm in perms {
        let ranges = perm["ipRanges"].as_array().cloned().unwrap_or_default();
        match existing.iter_mut().find(|e| same_rule(e, &perm)) {
            Some(e) => {
                let mut current = e["ipRanges"].as_array().cloned().unwrap_or_default();
                for r in &ranges {
                    let present = current.iter().any(|c| c["cidrIp"] == r["cidrIp"]);
                    if add && present {
                        return Err(err(
                            "InvalidPermission.Duplicate",
                            format!(
                                "the specified rule \"peer: {}, {}, ALLOW\" already exists",
                                s(r, "cidrIp"),
                                rule_label(&perm)
                            ),
                        ));
                    }
                    if !add && !present {
                        return Err(err(
                            "InvalidPermission.NotFound",
                            "The specified rule does not exist in this security group.",
                        ));
                    }
                    if add {
                        current.push(r.clone());
                    } else {
                        current.retain(|c| c["cidrIp"] != r["cidrIp"]);
                    }
                }
                e["ipRanges"] = Value::Array(current);
            }
            None if add => existing.push(perm.clone()),
            None => {
                return Err(err(
                    "InvalidPermission.NotFound",
                    "The specified rule does not exist in this security group.",
                ));
            }
        }
        for r in &ranges {
            let rule_id = format!("sgr-{}", hex_id(ctx, "sg-rule"));
            rule_items += &el(
                "item",
                &format!(
                    "{}{}{}{}{}{}{}{}",
                    tel("securityGroupRuleId", &rule_id),
                    tel("groupId", &id),
                    tel("groupOwnerId", &ctx.account),
                    el("isEgress", &(field == "ipPermissionsEgress").to_string()),
                    tel("ipProtocol", &s(&perm, "ipProtocol")),
                    el(
                        "fromPort",
                        &perm
                            .get("fromPort")
                            .map(Value::to_string)
                            .unwrap_or("-1".into())
                    ),
                    el(
                        "toPort",
                        &perm
                            .get("toPort")
                            .map(Value::to_string)
                            .unwrap_or("-1".into())
                    ),
                    tel("cidrIpv4", &s(r, "cidrIp"))
                ),
            );
        }
    }
    existing.retain(|e| {
        !e["ipRanges"].as_array().is_some_and(Vec::is_empty)
            || !e["groups"].as_array().is_none_or(Vec::is_empty)
    });
    group[field] = Value::Array(existing);
    ctx.put("security_groups", &id, group);
    if add {
        Ok(format!(
            "<return>true</return>{}",
            el("securityGroupRuleSet", &rule_items)
        ))
    } else {
        Ok("<return>true</return>".into())
    }
}

fn rule_label(perm: &Value) -> String {
    let proto = s(perm, "ipProtocol");
    match (perm.get("fromPort"), perm.get("toPort")) {
        (Some(f), Some(t)) if f == t => format!("{proto} port {f}"),
        (Some(f), Some(t)) => format!("{proto} ports {f}-{t}"),
        _ => proto,
    }
}

pub const REGIONS: [&str; 17] = [
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

fn describe_regions(p: &Params) -> String {
    let wanted = p.list("RegionName");
    let items: String = REGIONS
        .iter()
        .filter(|r| wanted.is_empty() || wanted.iter().any(|w| w == *r))
        .map(|r| {
            el(
                "item",
                &format!(
                    "{}{}{}",
                    tel("regionName", r),
                    tel("regionEndpoint", &format!("ec2.{r}.amazonaws.com")),
                    tel("optInStatus", "opt-in-not-required")
                ),
            )
        })
        .collect();
    el("regionInfo", &items)
}

fn describe_azs(ctx: &Ctx) -> String {
    let ids = [
        "use1-az1", "use1-az2", "use1-az4", "use1-az6", "use1-az3", "use1-az5",
    ];
    let items: String = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .zip(ids)
        .map(|(suffix, id)| {
            el(
                "item",
                &format!(
                    "{}{}{}{}{}",
                    tel("zoneName", &format!("{}{suffix}", ctx.region)),
                    tel("zoneState", "available"),
                    tel("regionName", &ctx.region),
                    tel("zoneId", id),
                    tel("zoneType", "availability-zone")
                ),
            )
        })
        .collect();
    el("availabilityZoneInfo", &items)
}
