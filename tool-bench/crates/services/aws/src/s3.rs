//! S3 (REST-XML, path-style: botocore keeps path style for `http://world:8116`).
//!
//! - `buckets` keyed by name: `Name`, `CreationDate`, `_region`, and per-bucket
//!   configuration: `Encryption` (absent = the SSE-S3 default every bucket has since 2023,
//!   `null` = no default encryption), `Versioning`, `PublicAccessBlock`, `Policy` (object),
//!   `Tags`, `Logging`, `OwnershipControls`, raw-XML `LifecycleXml`/`WebsiteXml`/`CorsXml`.
//! - `objects` keyed by `"<bucket>/<key>"`: `Bucket`, `Key`, `Size`, `LastModified`,
//!   `ETag`, `StorageClass`, `ContentType`, `Body` (text), `Tags`.

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};

use crate::{
    Ctx, Req,
    util::{
        Node, el, esc, iso_ms, parse_xml, percent_decode, respond, s, sha256_hex, tel, value_time,
        with_header, xml_response,
    },
};

const NS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

fn error(ctx: &mut Ctx, status: StatusCode, code: &str, msg: &str, extra: &str) -> Response {
    let rid = ctx.request_id();
    let rid = rid.replace('-', "")[..16].to_ascii_uppercase();
    xml_response(
        status,
        format!(
            "<Error><Code>{code}</Code><Message>{}</Message>{extra}<RequestId>{rid}</RequestId><HostId>toolbench/{rid}</HostId></Error>",
            esc(msg)
        ),
    )
}

fn no_bucket(ctx: &mut Ctx, bucket: &str) -> Response {
    error(
        ctx,
        StatusCode::NOT_FOUND,
        "NoSuchBucket",
        "The specified bucket does not exist",
        &tel("BucketName", bucket),
    )
}

fn ok_xml(body: String) -> Response {
    xml_response(StatusCode::OK, body)
}

fn empty(status: StatusCode) -> Response {
    respond(status, "application/xml", "")
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let path = req.path.trim_start_matches('/');
    let (bucket, key) = match path.split_once('/') {
        Some((b, k)) => (b.to_owned(), k.to_owned()),
        None => (path.to_owned(), String::new()),
    };
    if bucket.is_empty() {
        return match req.method {
            Method::GET => list_buckets(ctx, req),
            _ => error(
                ctx,
                StatusCode::METHOD_NOT_ALLOWED,
                "MethodNotAllowed",
                "The specified method is not allowed against this resource.",
                "",
            ),
        };
    }
    if key.is_empty() {
        bucket_op(ctx, req, &bucket)
    } else {
        object_op(ctx, req, &bucket, &key)
    }
}

fn owner_xml(ctx: &Ctx) -> String {
    let id = sha256_hex(ctx.account.as_bytes());
    format!(
        "<Owner>{}{}</Owner>",
        tel("ID", &id),
        tel("DisplayName", "toolbench-owner")
    )
}

fn bucket_region(b: &Value) -> String {
    b.get("_region")
        .and_then(Value::as_str)
        .unwrap_or("us-east-1")
        .to_owned()
}

fn list_buckets(ctx: &mut Ctx, req: &Req) -> Response {
    let prefix = req.q("prefix").unwrap_or_default();
    let items: String = ctx
        .docs("buckets")
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, b)| {
            el(
                "Bucket",
                &format!(
                    "{}{}{}",
                    tel("Name", name),
                    tel(
                        "CreationDate",
                        &b.get("CreationDate")
                            .and_then(value_time)
                            .map(iso_ms)
                            .unwrap_or_default()
                    ),
                    tel("BucketRegion", &bucket_region(b))
                ),
            )
        })
        .collect();
    ok_xml(format!(
        "<ListAllMyBucketsResult xmlns=\"{NS}\">{}<Buckets>{items}</Buckets></ListAllMyBucketsResult>",
        owner_xml(ctx)
    ))
}

fn bucket_op(ctx: &mut Ctx, req: &Req, bucket: &str) -> Response {
    let exists = ctx.get("buckets", bucket);
    match (&req.method, exists) {
        (&Method::PUT, None) if req.query.is_empty() => create_bucket(ctx, req, bucket),
        (&Method::PUT, Some(_)) if req.query.is_empty() => error(
            ctx,
            StatusCode::CONFLICT,
            "BucketAlreadyOwnedByYou",
            "Your previous request to create the named bucket succeeded and you already own it.",
            &tel("BucketName", bucket),
        ),
        (&Method::HEAD, None) => empty(StatusCode::NOT_FOUND),
        (_, None) => no_bucket(ctx, bucket),
        (&Method::HEAD, Some(b)) => with_header(
            empty(StatusCode::OK),
            "x-amz-bucket-region",
            &bucket_region(&b),
        ),
        (&Method::GET, Some(b)) => bucket_get(ctx, req, bucket, &b),
        (&Method::PUT, Some(b)) => bucket_put(ctx, req, bucket, b),
        (&Method::DELETE, Some(b)) => bucket_delete(ctx, req, bucket, b),
        (&Method::POST, Some(_)) if req.has_q("delete") => delete_objects(ctx, req, bucket),
        _ => error(
            ctx,
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "The specified method is not allowed against this resource.",
            "",
        ),
    }
}

fn create_bucket(ctx: &mut Ctx, req: &Req, bucket: &str) -> Response {
    let valid = (3..=63).contains(&bucket.len())
        && bucket
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && bucket
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if !valid {
        return error(
            ctx,
            StatusCode::BAD_REQUEST,
            "InvalidBucketName",
            "The specified bucket is not valid.",
            &tel("BucketName", bucket),
        );
    }
    let region = parse_xml(&req.body_text())
        .and_then(|n| n.text_of("LocationConstraint"))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "us-east-1".to_owned());
    ctx.put(
        "buckets",
        bucket,
        json!({"Name": bucket, "CreationDate": crate::util::iso(ctx.now), "_region": region}),
    );
    with_header(empty(StatusCode::OK), "location", &format!("/{bucket}"))
}

fn not_configured(ctx: &mut Ctx, code: &str, msg: &str, bucket: &str) -> Response {
    error(
        ctx,
        StatusCode::NOT_FOUND,
        code,
        msg,
        &tel("BucketName", bucket),
    )
}

fn tagset_xml(tags: &Value) -> String {
    let items: String = tags
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            el(
                "Tag",
                &format!(
                    "{}{}",
                    tel("Key", &s(t, "Key")),
                    tel("Value", &s(t, "Value"))
                ),
            )
        })
        .collect();
    format!("<Tagging xmlns=\"{NS}\"><TagSet>{items}</TagSet></Tagging>")
}

fn bool_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .to_string()
}

fn bucket_get(ctx: &mut Ctx, req: &Req, bucket: &str, b: &Value) -> Response {
    let sub = |k: &str| req.has_q(k);
    if sub("location") {
        let region = bucket_region(b);
        let loc = if region == "us-east-1" {
            String::new()
        } else {
            esc(&region)
        };
        return ok_xml(format!(
            "<LocationConstraint xmlns=\"{NS}\">{loc}</LocationConstraint>"
        ));
    }
    if sub("encryption") {
        return match b.get("Encryption") {
            Some(Value::Null) => not_configured(
                ctx,
                "ServerSideEncryptionConfigurationNotFoundError",
                "The server side encryption configuration was not found",
                bucket,
            ),
            other => {
                let default = json!({"SSEAlgorithm": "AES256", "BucketKeyEnabled": false});
                let e = other.unwrap_or(&default);
                let kms = e
                    .get("KMSMasterKeyID")
                    .and_then(Value::as_str)
                    .map(|k| tel("KMSMasterKeyID", k))
                    .unwrap_or_default();
                ok_xml(format!(
                    "<ServerSideEncryptionConfiguration xmlns=\"{NS}\"><Rule><ApplyServerSideEncryptionByDefault>{}{kms}</ApplyServerSideEncryptionByDefault>{}</Rule></ServerSideEncryptionConfiguration>",
                    tel("SSEAlgorithm", &s(e, "SSEAlgorithm")),
                    el("BucketKeyEnabled", &bool_str(e, "BucketKeyEnabled"))
                ))
            }
        };
    }
    if sub("versioning") {
        let status = b
            .get("Versioning")
            .and_then(Value::as_str)
            .map(|v| tel("Status", v))
            .unwrap_or_default();
        let mfa = b
            .get("MFADelete")
            .and_then(Value::as_str)
            .map(|v| tel("MfaDelete", v))
            .unwrap_or_default();
        return ok_xml(format!(
            "<VersioningConfiguration xmlns=\"{NS}\">{status}{mfa}</VersioningConfiguration>"
        ));
    }
    if sub("publicAccessBlock") {
        return match b.get("PublicAccessBlock").filter(|v| v.is_object()) {
            Some(pab) => ok_xml(format!(
                "<PublicAccessBlockConfiguration xmlns=\"{NS}\">{}{}{}{}</PublicAccessBlockConfiguration>",
                el("BlockPublicAcls", &bool_str(pab, "BlockPublicAcls")),
                el("IgnorePublicAcls", &bool_str(pab, "IgnorePublicAcls")),
                el("BlockPublicPolicy", &bool_str(pab, "BlockPublicPolicy")),
                el(
                    "RestrictPublicBuckets",
                    &bool_str(pab, "RestrictPublicBuckets")
                )
            )),
            None => not_configured(
                ctx,
                "NoSuchPublicAccessBlockConfiguration",
                "The public access block configuration was not found",
                bucket,
            ),
        };
    }
    if sub("policyStatus") {
        return match b.get("Policy").filter(|v| v.is_object()) {
            Some(policy) => ok_xml(format!(
                "<PolicyStatus xmlns=\"{NS}\"><IsPublic>{}</IsPublic></PolicyStatus>",
                policy_is_public(policy)
            )),
            None => not_configured(
                ctx,
                "NoSuchBucketPolicy",
                "The bucket policy does not exist",
                bucket,
            ),
        };
    }
    if sub("policy") {
        return match b.get("Policy").filter(|v| !v.is_null()) {
            Some(policy) => respond(
                StatusCode::OK,
                "application/json",
                match policy {
                    Value::String(t) => t.clone(),
                    other => other.to_string(),
                },
            ),
            None => not_configured(
                ctx,
                "NoSuchBucketPolicy",
                "The bucket policy does not exist",
                bucket,
            ),
        };
    }
    if sub("tagging") {
        return match b
            .get("Tags")
            .filter(|t| t.as_array().is_some_and(|a| !a.is_empty()))
        {
            Some(tags) => ok_xml(tagset_xml(tags)),
            None => not_configured(ctx, "NoSuchTagSet", "The TagSet does not exist", bucket),
        };
    }
    if sub("acl") {
        return ok_xml(acl_xml(ctx));
    }
    if sub("logging") {
        let inner = b
            .get("Logging")
            .filter(|l| l.is_object())
            .map(|l| {
                el(
                    "LoggingEnabled",
                    &format!(
                        "{}{}",
                        tel("TargetBucket", &s(l, "TargetBucket")),
                        tel("TargetPrefix", &s(l, "TargetPrefix"))
                    ),
                )
            })
            .unwrap_or_default();
        return ok_xml(format!(
            "<BucketLoggingStatus xmlns=\"{NS}\">{inner}</BucketLoggingStatus>"
        ));
    }
    if sub("ownershipControls") {
        let rule = b
            .get("OwnershipControls")
            .and_then(Value::as_str)
            .unwrap_or("BucketOwnerEnforced");
        return ok_xml(format!(
            "<OwnershipControls xmlns=\"{NS}\"><Rule><ObjectOwnership>{rule}</ObjectOwnership></Rule></OwnershipControls>"
        ));
    }
    for (q, field, code, msg) in [
        (
            "lifecycle",
            "LifecycleXml",
            "NoSuchLifecycleConfiguration",
            "The lifecycle configuration does not exist",
        ),
        (
            "website",
            "WebsiteXml",
            "NoSuchWebsiteConfiguration",
            "The specified bucket does not have a website configuration",
        ),
        (
            "cors",
            "CorsXml",
            "NoSuchCORSConfiguration",
            "The CORS configuration does not exist",
        ),
        (
            "replication",
            "ReplicationXml",
            "ReplicationConfigurationNotFoundError",
            "The replication configuration was not found",
        ),
        (
            "object-lock",
            "ObjectLockXml",
            "ObjectLockConfigurationNotFoundError",
            "Object Lock configuration does not exist for this bucket",
        ),
    ] {
        if sub(q) {
            return match b.get(field).and_then(Value::as_str) {
                Some(xml) => ok_xml(xml.to_owned()),
                None => not_configured(ctx, code, msg, bucket),
            };
        }
    }
    if sub("notification") {
        return ok_xml(format!("<NotificationConfiguration xmlns=\"{NS}\"/>"));
    }
    if sub("accelerate") {
        return ok_xml(format!("<AccelerateConfiguration xmlns=\"{NS}\"/>"));
    }
    if sub("requestPayment") {
        return ok_xml(format!(
            "<RequestPaymentConfiguration xmlns=\"{NS}\"><Payer>BucketOwner</Payer></RequestPaymentConfiguration>"
        ));
    }
    if sub("uploads") {
        return ok_xml(format!(
            "<ListMultipartUploadsResult xmlns=\"{NS}\">{}<KeyMarker></KeyMarker><UploadIdMarker></UploadIdMarker><MaxUploads>1000</MaxUploads><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>",
            tel("Bucket", bucket)
        ));
    }
    if sub("analytics") || sub("metrics") || sub("inventory") || sub("intelligent-tiering") {
        let root = if sub("analytics") {
            "ListBucketAnalyticsConfigurationResult"
        } else if sub("metrics") {
            "ListMetricsConfigurationsResult"
        } else if sub("inventory") {
            "ListInventoryConfigurationsResult"
        } else {
            "ListBucketIntelligentTieringConfigurationsOutput"
        };
        return ok_xml(format!(
            "<{root} xmlns=\"{NS}\"><IsTruncated>false</IsTruncated></{root}>"
        ));
    }
    if sub("versions") {
        return list_versions(ctx, req, bucket);
    }
    list_objects(ctx, req, bucket)
}

fn acl_xml(ctx: &Ctx) -> String {
    let id = sha256_hex(ctx.account.as_bytes());
    format!(
        "<AccessControlPolicy xmlns=\"{NS}\">{}<AccessControlList><Grant><Grantee xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"CanonicalUser\">{}{}</Grantee><Permission>FULL_CONTROL</Permission></Grant></AccessControlList></AccessControlPolicy>",
        owner_xml(ctx),
        tel("ID", &id),
        tel("DisplayName", "toolbench-owner")
    )
}

fn policy_is_public(policy: &Value) -> bool {
    let statements = match policy.get("Statement") {
        Some(Value::Array(a)) => a.clone(),
        Some(o) => vec![o.clone()],
        None => Vec::new(),
    };
    statements.iter().any(|st| {
        st["Effect"] == "Allow"
            && st.get("Condition").is_none()
            && (st["Principal"] == "*" || st["Principal"]["AWS"] == "*")
    })
}

fn objects_in(ctx: &Ctx, bucket: &str) -> Vec<Value> {
    let prefix = format!("{bucket}/");
    let mut objs: Vec<Value> = ctx
        .docs("objects")
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| v)
        .collect();
    objs.sort_by_key(|o| s(o, "Key"));
    objs
}

fn object_xml(o: &Value, owner: Option<&str>) -> String {
    format!(
        "{}{}{}{}{}{}",
        tel("Key", &s(o, "Key")),
        tel(
            "LastModified",
            &o.get("LastModified")
                .and_then(value_time)
                .map(iso_ms)
                .unwrap_or_default()
        ),
        tel("ETag", &format!("\"{}\"", etag_of(o))),
        el("Size", &size_of(o).to_string()),
        tel(
            "StorageClass",
            o.get("StorageClass")
                .and_then(Value::as_str)
                .unwrap_or("STANDARD")
        ),
        owner.unwrap_or_default()
    )
}

fn size_of(o: &Value) -> u64 {
    o.get("Size")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| s(o, "Body").len() as u64)
}

fn etag_of(o: &Value) -> String {
    o.get("ETag")
        .and_then(Value::as_str)
        .map(|e| e.trim_matches('"').to_owned())
        .unwrap_or_else(|| etag(s(o, "Body").as_bytes()))
}

fn etag(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..32].to_owned()
}

/// Keys and common prefixes after prefix/delimiter grouping, in key order.
fn listing(objs: &[Value], prefix: &str, delimiter: Option<&str>) -> Vec<(String, Option<Value>)> {
    let mut out: Vec<(String, Option<Value>)> = Vec::new();
    for o in objs {
        let key = s(o, "Key");
        if !key.starts_with(prefix) {
            continue;
        }
        if let Some(d) = delimiter.filter(|d| !d.is_empty())
            && let Some(pos) = key[prefix.len()..].find(d)
        {
            let cp = key[..prefix.len() + pos + d.len()].to_owned();
            if !out.iter().any(|(k, v)| v.is_none() && *k == cp) {
                out.push((cp, None));
            }
            continue;
        }
        out.push((key, Some(o.clone())));
    }
    out
}

fn list_objects(ctx: &mut Ctx, req: &Req, bucket: &str) -> Response {
    let v2 = req.q("list-type") == Some("2");
    let prefix = req.q("prefix").unwrap_or_default().to_owned();
    let delimiter = req.q("delimiter").map(str::to_owned);
    let max_keys: usize = req
        .q("max-keys")
        .and_then(|m| m.parse().ok())
        .unwrap_or(1000)
        .min(1000);
    let start_after = if v2 {
        req.q("continuation-token")
            .map(|t| percent_decode(t, false))
            .or_else(|| req.q("start-after").map(str::to_owned))
    } else {
        req.q("marker").map(str::to_owned)
    }
    .unwrap_or_default();
    let objs = objects_in(ctx, bucket);
    let all = listing(&objs, &prefix, delimiter.as_deref());
    let remaining: Vec<&(String, Option<Value>)> = all
        .iter()
        .filter(|(k, _)| start_after.is_empty() || *k > start_after)
        .collect();
    let page: Vec<&&(String, Option<Value>)> = remaining.iter().take(max_keys).collect();
    let truncated = remaining.len() > max_keys;
    let mut x = format!(
        "<ListBucketResult xmlns=\"{NS}\">{}{}",
        tel("Name", bucket),
        tel("Prefix", &prefix)
    );
    if v2 {
        if let Some(t) = req.q("continuation-token") {
            x += &tel("ContinuationToken", t);
        }
        if let Some(sa) = req.q("start-after") {
            x += &tel("StartAfter", sa);
        }
        x += &el("KeyCount", &page.len().to_string());
    } else {
        x += &tel("Marker", req.q("marker").unwrap_or_default());
    }
    x += &el("MaxKeys", &max_keys.to_string());
    if let Some(d) = &delimiter {
        x += &tel("Delimiter", d);
    }
    x += &el("IsTruncated", &truncated.to_string());
    if truncated && let Some((last, _)) = page.last().map(|p| &***p) {
        if v2 {
            x += &tel("NextContinuationToken", last);
        } else if delimiter.is_some() {
            x += &tel("NextMarker", last);
        }
    }
    let owner = if v2 {
        req.q("fetch-owner")
            .filter(|v| *v == "true")
            .map(|_| owner_xml(ctx))
    } else {
        Some(owner_xml(ctx))
    };
    for (k, o) in &page {
        match o {
            Some(o) => x += &el("Contents", &object_xml(o, owner.as_deref())),
            None => x += &el("CommonPrefixes", &tel("Prefix", k)),
        }
    }
    x += "</ListBucketResult>";
    ok_xml(x)
}

fn list_versions(ctx: &mut Ctx, req: &Req, bucket: &str) -> Response {
    let prefix = req.q("prefix").unwrap_or_default();
    let objs = objects_in(ctx, bucket);
    let owner = owner_xml(ctx);
    let items: String = objs
        .iter()
        .filter(|o| s(o, "Key").starts_with(prefix))
        .map(|o| {
            el(
                "Version",
                &format!(
                    "{}{}{}{}{}{}{}{owner}",
                    tel("Key", &s(o, "Key")),
                    tel(
                        "VersionId",
                        o.get("VersionId").and_then(Value::as_str).unwrap_or("null")
                    ),
                    "<IsLatest>true</IsLatest>",
                    tel(
                        "LastModified",
                        &o.get("LastModified")
                            .and_then(value_time)
                            .map(iso_ms)
                            .unwrap_or_default()
                    ),
                    tel("ETag", &format!("\"{}\"", etag_of(o))),
                    el("Size", &size_of(o).to_string()),
                    tel("StorageClass", "STANDARD")
                ),
            )
        })
        .collect();
    ok_xml(format!(
        "<ListVersionsResult xmlns=\"{NS}\">{}{}<KeyMarker></KeyMarker><VersionIdMarker></VersionIdMarker><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{items}</ListVersionsResult>",
        tel("Name", bucket),
        tel("Prefix", prefix)
    ))
}

fn xml_bool(n: &Node, key: &str) -> bool {
    n.text_of(key).is_some_and(|v| v == "true")
}

fn bucket_put(ctx: &mut Ctx, req: &Req, bucket: &str, mut b: Value) -> Response {
    let body = decode_body(req);
    let text = String::from_utf8_lossy(&body).into_owned();
    let node = parse_xml(&text);
    let malformed = |ctx: &mut Ctx| {
        error(
            ctx,
            StatusCode::BAD_REQUEST,
            "MalformedXML",
            "The XML you provided was not well-formed or did not validate against our published schema",
            "",
        )
    };
    if req.has_q("encryption") {
        let Some(rule) = node.as_ref().and_then(|n| n.child("Rule")).cloned() else {
            return malformed(ctx);
        };
        let def = rule
            .child("ApplyServerSideEncryptionByDefault")
            .cloned()
            .unwrap_or_default();
        let alg = def.text_of("SSEAlgorithm").unwrap_or_default();
        if !["AES256", "aws:kms", "aws:kms:dsse"].contains(&alg.as_str()) {
            return malformed(ctx);
        }
        let mut e =
            json!({"SSEAlgorithm": alg, "BucketKeyEnabled": xml_bool(&rule, "BucketKeyEnabled")});
        if let Some(k) = def.text_of("KMSMasterKeyID") {
            e["KMSMasterKeyID"] = json!(k);
        }
        b["Encryption"] = e;
    } else if req.has_q("versioning") {
        let Some(status) = node.as_ref().and_then(|n| n.text_of("Status")) else {
            return malformed(ctx);
        };
        if status != "Enabled" && status != "Suspended" {
            return malformed(ctx);
        }
        b["Versioning"] = json!(status);
    } else if req.has_q("publicAccessBlock") {
        let Some(n) = node else { return malformed(ctx) };
        b["PublicAccessBlock"] = json!({
            "BlockPublicAcls": xml_bool(&n, "BlockPublicAcls"),
            "IgnorePublicAcls": xml_bool(&n, "IgnorePublicAcls"),
            "BlockPublicPolicy": xml_bool(&n, "BlockPublicPolicy"),
            "RestrictPublicBuckets": xml_bool(&n, "RestrictPublicBuckets"),
        });
    } else if req.has_q("policy") {
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.get("Statement").is_some() => b["Policy"] = v,
            _ => {
                return error(
                    ctx,
                    StatusCode::BAD_REQUEST,
                    "MalformedPolicy",
                    "Policies must be valid JSON and the first byte must be '{'",
                    "",
                );
            }
        }
    } else if req.has_q("tagging") {
        let Some(n) = node else { return malformed(ctx) };
        b["Tags"] = tags_from(&n);
    } else if req.has_q("logging") {
        let enabled = node
            .as_ref()
            .and_then(|n| n.child("LoggingEnabled"))
            .cloned();
        b["Logging"] = match enabled {
            Some(l) => {
                json!({"TargetBucket": l.text_of("TargetBucket"), "TargetPrefix": l.text_of("TargetPrefix").unwrap_or_default()})
            }
            None => Value::Null,
        };
    } else if req.has_q("ownershipControls") {
        let rule = node
            .as_ref()
            .and_then(|n| n.child("Rule"))
            .and_then(|r| r.text_of("ObjectOwnership"));
        b["OwnershipControls"] = json!(rule);
    } else if req.has_q("lifecycle") {
        b["LifecycleXml"] = json!(text);
    } else if req.has_q("website") {
        b["WebsiteXml"] = json!(text);
    } else if req.has_q("cors") {
        b["CorsXml"] = json!(text);
    } else if req.has_q("acl") || req.has_q("notification") || req.has_q("accelerate") {
        // Accepted; the mock keeps the default ACL/notification state.
    } else {
        return error(
            ctx,
            StatusCode::NOT_IMPLEMENTED,
            "NotImplemented",
            "A header you provided implies functionality that is not implemented",
            "",
        );
    }
    ctx.put("buckets", bucket, b);
    empty(StatusCode::OK)
}

fn tags_from(n: &Node) -> Value {
    let set = n.child("TagSet").cloned().unwrap_or_default();
    Value::Array(
        set.all("Tag")
            .iter()
            .map(|t| json!({"Key": t.text_of("Key").unwrap_or_default(), "Value": t.text_of("Value").unwrap_or_default()}))
            .collect(),
    )
}

fn bucket_delete(ctx: &mut Ctx, req: &Req, bucket: &str, mut b: Value) -> Response {
    let field = if req.has_q("encryption") {
        Some("Encryption")
    } else if req.has_q("publicAccessBlock") {
        Some("PublicAccessBlock")
    } else if req.has_q("policy") {
        Some("Policy")
    } else if req.has_q("tagging") {
        Some("Tags")
    } else if req.has_q("lifecycle") {
        Some("LifecycleXml")
    } else if req.has_q("website") {
        Some("WebsiteXml")
    } else if req.has_q("cors") {
        Some("CorsXml")
    } else if req.has_q("ownershipControls") {
        Some("OwnershipControls")
    } else {
        None
    };
    match field {
        Some("Encryption") => {
            // Deleting the configuration reverts to the SSE-S3 default.
            if let Some(o) = b.as_object_mut() {
                o.shift_remove("Encryption");
            }
        }
        Some(f) => {
            if let Some(o) = b.as_object_mut() {
                o.shift_remove(f);
            }
        }
        None => {
            if !objects_in(ctx, bucket).is_empty() {
                return error(
                    ctx,
                    StatusCode::CONFLICT,
                    "BucketNotEmpty",
                    "The bucket you tried to delete is not empty",
                    &tel("BucketName", bucket),
                );
            }
            ctx.remove("buckets", bucket);
            return empty(StatusCode::NO_CONTENT);
        }
    }
    ctx.put("buckets", bucket, b);
    empty(StatusCode::NO_CONTENT)
}

fn delete_objects(ctx: &mut Ctx, req: &Req, bucket: &str) -> Response {
    let body = decode_body(req);
    let Some(n) = parse_xml(&String::from_utf8_lossy(&body)) else {
        return error(
            ctx,
            StatusCode::BAD_REQUEST,
            "MalformedXML",
            "The XML you provided was not well-formed or did not validate against our published schema",
            "",
        );
    };
    let quiet = n.text_of("Quiet").is_some_and(|q| q == "true");
    let mut deleted = String::new();
    for o in n.all("Object") {
        let key = o.text_of("Key").unwrap_or_default();
        ctx.remove("objects", &format!("{bucket}/{key}"));
        if !quiet {
            deleted += &el("Deleted", &tel("Key", &key));
        }
    }
    ok_xml(format!(
        "<DeleteResult xmlns=\"{NS}\">{deleted}</DeleteResult>"
    ))
}

/// Request body with `aws-chunked` framing removed.
fn decode_body(req: &Req) -> Vec<u8> {
    let chunked = req
        .header("content-encoding")
        .is_some_and(|e| e.contains("aws-chunked"))
        || req
            .header("x-amz-content-sha256")
            .is_some_and(|h| h.starts_with("STREAMING-"));
    if !chunked {
        return req.body.to_vec();
    }
    let mut out = Vec::new();
    let mut rest: &[u8] = &req.body;
    while let Some(pos) = rest.windows(2).position(|w| w == b"\r\n") {
        let header = String::from_utf8_lossy(&rest[..pos]);
        let size =
            usize::from_str_radix(header.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        rest = &rest[pos + 2..];
        if size == 0 || rest.len() < size {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        }
    }
    out
}

pub fn http_date(secs: i64) -> String {
    let dt = time::OffsetDateTime::from_unix_timestamp(secs).expect("valid timestamp");
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAYS[dt.weekday().number_days_from_monday() as usize],
        dt.day(),
        MONTHS[dt.month() as usize - 1],
        dt.year(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

fn object_op(ctx: &mut Ctx, req: &Req, bucket: &str, key: &str) -> Response {
    let Some(b) = ctx.get("buckets", bucket) else {
        if req.method == Method::HEAD {
            return empty(StatusCode::NOT_FOUND);
        }
        return no_bucket(ctx, bucket);
    };
    let id = format!("{bucket}/{key}");
    let existing = ctx.get("objects", &id);
    let no_key = |ctx: &mut Ctx| {
        error(
            ctx,
            StatusCode::NOT_FOUND,
            "NoSuchKey",
            "The specified key does not exist.",
            &tel("Key", key),
        )
    };
    match req.method {
        Method::GET | Method::HEAD => {
            let Some(o) = existing else {
                if req.method == Method::HEAD {
                    return empty(StatusCode::NOT_FOUND);
                }
                return no_key(ctx);
            };
            if req.has_q("tagging") {
                return ok_xml(tagset_xml(o.get("Tags").unwrap_or(&json!([]))));
            }
            if req.has_q("acl") {
                return ok_xml(acl_xml(ctx));
            }
            let body = if req.method == Method::HEAD {
                String::new()
            } else {
                s(&o, "Body")
            };
            let mut resp = respond(
                StatusCode::OK,
                o.get("ContentType")
                    .and_then(Value::as_str)
                    .unwrap_or("binary/octet-stream"),
                body,
            );
            resp = with_header(resp, "etag", &format!("\"{}\"", etag_of(&o)));
            resp = with_header(
                resp,
                "last-modified",
                &http_date(
                    o.get("LastModified")
                        .and_then(value_time)
                        .unwrap_or(ctx.now),
                ),
            );
            if req.method == Method::HEAD {
                resp = with_header(resp, "content-length", &size_of(&o).to_string());
            }
            let enc = match b.get("Encryption") {
                Some(Value::Null) => None,
                Some(e) => Some(s(e, "SSEAlgorithm")),
                None => Some("AES256".to_owned()),
            };
            if let Some(alg) = enc {
                resp = with_header(resp, "x-amz-server-side-encryption", &alg);
            }
            with_header(resp, "accept-ranges", "bytes")
        }
        Method::PUT => {
            if req.has_q("tagging") {
                let Some(mut o) = existing else {
                    return no_key(ctx);
                };
                let n = parse_xml(&String::from_utf8_lossy(&decode_body(req))).unwrap_or_default();
                o["Tags"] = tags_from(&n);
                ctx.put("objects", &id, o);
                return empty(StatusCode::OK);
            }
            if req.has_q("acl") {
                return empty(StatusCode::OK);
            }
            let (bytes, content_type) = if let Some(src) = req.header("x-amz-copy-source") {
                let src = percent_decode(src.trim_start_matches('/'), false);
                let src = src.split('?').next().unwrap_or_default().to_owned();
                match ctx.get("objects", &src) {
                    Some(o) => (s(&o, "Body").into_bytes(), s(&o, "ContentType")),
                    None => {
                        return error(
                            ctx,
                            StatusCode::NOT_FOUND,
                            "NoSuchKey",
                            "The specified key does not exist.",
                            &tel("Key", src.split_once('/').map(|x| x.1).unwrap_or_default()),
                        );
                    }
                }
            } else {
                (
                    decode_body(req),
                    req.header("content-type")
                        .unwrap_or("binary/octet-stream")
                        .to_owned(),
                )
            };
            let tag = etag(&bytes);
            let doc = json!({
                "Bucket": bucket,
                "Key": key,
                "Size": bytes.len(),
                "LastModified": crate::util::iso(ctx.now),
                "ETag": tag,
                "StorageClass": req.header("x-amz-storage-class").unwrap_or("STANDARD"),
                "ContentType": content_type,
                "Body": String::from_utf8_lossy(&bytes),
            });
            ctx.put("objects", &id, doc);
            if req.header("x-amz-copy-source").is_some() {
                return ok_xml(format!(
                    "<CopyObjectResult xmlns=\"{NS}\">{}{}</CopyObjectResult>",
                    tel("LastModified", &iso_ms(ctx.now)),
                    tel("ETag", &format!("\"{tag}\""))
                ));
            }
            with_header(empty(StatusCode::OK), "etag", &format!("\"{tag}\""))
        }
        Method::DELETE => {
            if req.has_q("tagging") {
                if let Some(mut o) = existing {
                    o["Tags"] = json!([]);
                    ctx.put("objects", &id, o);
                }
                return empty(StatusCode::NO_CONTENT);
            }
            ctx.remove("objects", &id);
            empty(StatusCode::NO_CONTENT)
        }
        _ => error(
            ctx,
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "The specified method is not allowed against this resource.",
            "",
        ),
    }
}
