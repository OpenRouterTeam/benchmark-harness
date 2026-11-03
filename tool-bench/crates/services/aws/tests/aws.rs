use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_aws::{Aws, util::parse_xml};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use serde_json::{Value, json};
use tower::ServiceExt;

fn world(scenario: &str) -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios")
        .join(scenario);
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn app(w: &SharedWorld) -> Router {
    service_app(w.clone(), &Aws)
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    fn xml(&self) -> mock_aws::util::Node {
        parse_xml(&self.text).unwrap_or_else(|| panic!("not XML: {}", self.text))
    }
}

async fn send(app: &Router, req: Request<Body>) -> Resp {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    Resp {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn auth(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIATOOLBENCH0000000/20260314/us-east-1/{service}/aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
    )
}

/// Query protocol (IAM, STS, EC2): form body.
async fn query(app: &Router, service: &str, body: &str) -> Resp {
    let req = Request::post("/")
        .header("host", "world:8116")
        .header("authorization", auth(service))
        .header(
            "content-type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .body(Body::from(body.to_owned()))
        .unwrap();
    send(app, req).await
}

/// JSON protocols (CloudWatch 1.0, Logs 1.1).
async fn target(app: &Router, service: &str, target: &str, body: Value) -> Resp {
    let req = Request::post("/")
        .header("host", "world:8116")
        .header("authorization", auth(service))
        .header("x-amz-target", target)
        .header("content-type", "application/x-amz-json-1.0")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(app, req).await
}

/// REST protocols (S3, Lambda, Account).
async fn rest(app: &Router, service: &str, method: &str, uri: &str, body: &str) -> Resp {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "world:8116")
        .header("authorization", auth(service))
        .body(Body::from(body.to_owned()))
        .unwrap();
    send(app, req).await
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["aws"].clone()
}

// ───────────────────────────── service reference & helpers ─────────────────────────────

#[tokio::test]
async fn serves_the_service_reference_index_and_documents() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let index = send(&app, Request::get("/").body(Body::empty()).unwrap()).await;
    assert_eq!(index.status, StatusCode::OK);
    let services = index.json();
    assert!(services.as_array().unwrap().len() > 400);
    assert!(
        services
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["service"] == "ec2")
    );

    let req = Request::get("/v1/iam/iam.json")
        .header("host", "servicereference.us-east-1.amazonaws.com")
        .body(Body::empty())
        .unwrap();
    let iam = send(&app, req).await.json();
    let list_users = iam["Actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["Name"] == "ListUsers")
        .unwrap();
    assert_eq!(list_users["Annotations"]["Properties"]["IsWrite"], false);

    // An indexed service without a pinned copy: no actions (treated as not read-only).
    let req = Request::get("/v1/dynamodb/dynamodb.json")
        .header("host", "servicereference.us-east-1.amazonaws.com")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.json()["Actions"], json!([]));
    let req = Request::get("/v1/nope/nope.json")
        .header("host", "servicereference.us-east-1.amazonaws.com")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn suggestions_are_deterministic_and_relevant() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let req = || {
        Request::post("/suggest-aws-commands")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"query":"which S3 bucket is unencrypted"}"#))
            .unwrap()
    };
    let a = send(&app, req()).await;
    let b = send(&app, req()).await;
    assert_eq!(a.text, b.text);
    let first = &a.json()["suggestions"][0];
    assert_eq!(first["command"], "aws s3api get-bucket-encryption");
    assert!(first["confidence"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn unsigned_sdk_calls_are_rejected_and_dates_follow_the_logical_clock() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let resp = send(
        &app,
        Request::post("/")
            .body(Body::from("Action=ListUsers"))
            .unwrap(),
    )
    .await;
    assert_eq!(resp.status, StatusCode::FORBIDDEN);
    let resp = rest(&app, "s3", "GET", "/", "").await;
    // The rejected POST ticked the clock once.
    assert_eq!(resp.headers["date"], "Sat, 14 Mar 2026 09:00:07 GMT");
}

// ───────────────────────────── STS / IAM ─────────────────────────────

#[tokio::test]
async fn sts_reports_the_caller() {
    let w = world("cloud/aws/grant-auditor-access");
    let app = app(&w);
    let resp = query(&app, "sts", "Action=GetCallerIdentity&Version=2011-06-15").await;
    let x = resp.xml();
    let result = x.child("GetCallerIdentityResult").unwrap();
    assert_eq!(result.text_of("Account").unwrap(), "481562039417");
    assert_eq!(
        result.text_of("Arn").unwrap(),
        "arn:aws:iam::481562039417:user/platform-ops"
    );
}

#[tokio::test]
async fn iam_lists_users_with_marker_pagination() {
    let w = world("cloud/aws/grant-auditor-access");
    let app = app(&w);
    let page1 = query(
        &app,
        "iam",
        "Action=ListUsers&Version=2010-05-08&MaxItems=4",
    )
    .await;
    let r = page1.xml();
    let result = r.child("ListUsersResult").unwrap();
    assert_eq!(result.child("Users").unwrap().all("member").len(), 4);
    assert_eq!(result.text_of("IsTruncated").unwrap(), "true");
    let marker = result.text_of("Marker").unwrap();
    let page2 = query(
        &app,
        "iam",
        &format!("Action=ListUsers&Version=2010-05-08&MaxItems=4&Marker={marker}"),
    )
    .await;
    let r2 = page2.xml();
    let users = r2
        .child("ListUsersResult")
        .unwrap()
        .child("Users")
        .unwrap()
        .all("member")
        .len();
    assert_eq!(users, 2);

    let external = query(
        &app,
        "iam",
        "Action=ListUsers&Version=2010-05-08&PathPrefix=%2Fexternal%2F",
    )
    .await;
    assert!(external.text.contains("priya.raman-audit"));
    assert!(!external.text.contains("praman<"));
}

#[tokio::test]
async fn iam_errors_use_the_query_error_envelope() {
    let w = world("cloud/aws/grant-auditor-access");
    let app = app(&w);
    let resp = query(
        &app,
        "iam",
        "Action=GetUser&Version=2010-05-08&UserName=ghost",
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
    let err = resp.xml();
    let e = err.child("Error").unwrap();
    assert_eq!(e.text_of("Code").unwrap(), "NoSuchEntity");
    assert_eq!(
        e.text_of("Message").unwrap(),
        "The user with name ghost cannot be found."
    );

    let resp = query(
        &app,
        "iam",
        "Action=DeleteUser&Version=2010-05-08&UserName=svc-backup",
    )
    .await;
    assert_eq!(resp.status, StatusCode::CONFLICT);
    assert!(resp.text.contains("DeleteConflict"));

    let resp = query(
        &app,
        "iam",
        "Action=AttachUserPolicy&Version=2010-05-08&UserName=lnovak&PolicyArn=arn%3Aaws%3Aiam%3A%3Aaws%3Apolicy%2FNope",
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn iam_attach_and_group_membership_change_state() {
    let w = world("cloud/aws/grant-auditor-access");
    let app = app(&w);
    let resp = query(
        &app,
        "iam",
        "Action=AttachUserPolicy&Version=2010-05-08&UserName=priya.raman-audit&PolicyArn=arn%3Aaws%3Aiam%3A%3Aaws%3Apolicy%2FSecurityAudit",
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);
    query(
        &app,
        "iam",
        "Action=AddUserToGroup&Version=2010-05-08&GroupName=auditors&UserName=priya.raman-audit",
    )
    .await;
    let s = state(&w);
    assert_eq!(
        s["users"]["priya.raman-audit"]["AttachedPolicies"],
        json!(["arn:aws:iam::aws:policy/SecurityAudit"])
    );
    assert_eq!(
        s["groups"]["auditors"]["Members"],
        json!(["lnovak", "priya.raman-audit"])
    );

    let attached = query(
        &app,
        "iam",
        "Action=ListAttachedUserPolicies&Version=2010-05-08&UserName=priya.raman-audit",
    )
    .await;
    assert!(
        attached
            .text
            .contains("<PolicyName>SecurityAudit</PolicyName>")
    );
    let groups = query(
        &app,
        "iam",
        "Action=ListGroupsForUser&Version=2010-05-08&UserName=priya.raman-audit",
    )
    .await;
    assert!(groups.text.contains("<GroupName>auditors</GroupName>"));

    // Detach puts it back; detaching again is NoSuchEntity.
    let detach = "Action=DetachUserPolicy&Version=2010-05-08&UserName=priya.raman-audit&PolicyArn=arn%3Aaws%3Aiam%3A%3Aaws%3Apolicy%2FSecurityAudit";
    assert_eq!(query(&app, "iam", detach).await.status, StatusCode::OK);
    assert_eq!(
        query(&app, "iam", detach).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn iam_policy_documents_are_url_encoded_and_simulation_evaluates_them() {
    let w = world("cloud/aws/grant-auditor-access");
    let app = app(&w);
    let resp = query(
        &app,
        "iam",
        "Action=GetUserPolicy&Version=2010-05-08&UserName=svc-backup&PolicyName=backup-write",
    )
    .await;
    let doc = resp
        .xml()
        .child("GetUserPolicyResult")
        .unwrap()
        .text_of("PolicyDocument")
        .unwrap();
    assert!(doc.starts_with("%7B%22Version%22"));

    let sim = query(
        &app,
        "iam",
        "Action=SimulatePrincipalPolicy&Version=2010-05-08&PolicySourceArn=arn%3Aaws%3Aiam%3A%3A481562039417%3Auser%2Fservice%2Fsvc-backup&ActionNames.member.1=s3%3APutObject&ActionNames.member.2=s3%3ADeleteObject&ResourceArns.member.1=arn%3Aaws%3As3%3A%3A%3Anorthwind-backups%2Fdaily.tar",
    )
    .await;
    let x = sim.xml();
    let results = x
        .child("SimulatePrincipalPolicyResult")
        .unwrap()
        .child("EvaluationResults")
        .unwrap()
        .all("member");
    assert_eq!(results[0].text_of("EvalDecision").unwrap(), "allowed");
    assert_eq!(results[1].text_of("EvalDecision").unwrap(), "implicitDeny");

    // Group policies apply to members.
    let sim = query(
        &app,
        "iam",
        "Action=SimulatePrincipalPolicy&Version=2010-05-08&PolicySourceArn=arn%3Aaws%3Aiam%3A%3A481562039417%3Auser%2Fjchen&ActionNames.member.1=iam%3ADeleteUser",
    )
    .await;
    assert!(sim.text.contains("<EvalDecision>allowed</EvalDecision>"));
}

#[tokio::test]
async fn iam_access_keys_are_created_deterministically_and_capped() {
    let a = world("cloud/aws/grant-auditor-access");
    let b = world("cloud/aws/grant-auditor-access");
    let body = "Action=CreateAccessKey&Version=2010-05-08&UserName=lnovak";
    let ka = query(&app(&a), "iam", body).await.text;
    let kb = query(&app(&b), "iam", body).await.text;
    assert_eq!(ka, kb);
    assert!(ka.contains("<AccessKeyId>AKIA"));
    let app = app(&a);
    query(&app, "iam", body).await;
    let third = query(&app, "iam", body).await;
    assert_eq!(third.status, StatusCode::CONFLICT);
    assert!(third.text.contains("LimitExceeded"));
}

// ───────────────────────────── EC2 ─────────────────────────────

#[tokio::test]
async fn ec2_filters_by_tag_and_groups_reservations() {
    let w = world("cloud/aws/stop-legacy-reporting-worker");
    let app = app(&w);
    let resp = query(
        &app,
        "ec2",
        "Action=DescribeInstances&Version=2016-11-15&Filter.1.Name=tag%3AName&Filter.1.Value.1=reporting-worker-legacy&Filter.2.Name=tag%3AEnvironment&Filter.2.Value.1=prod",
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);
    let x = resp.xml();
    let reservations = x.child("reservationSet").unwrap().all("item");
    assert_eq!(reservations.len(), 1);
    let inst = reservations[0].child("instancesSet").unwrap().all("item")[0];
    assert_eq!(inst.text_of("instanceId").unwrap(), "i-07b9d1f3a5c8e2046");

    let running = query(
        &app,
        "ec2",
        "Action=DescribeInstances&Version=2016-11-15&Filter.1.Name=instance-state-name&Filter.1.Value.1=stopped",
    )
    .await;
    assert!(running.text.contains("i-0c8e2a4f6b1d39057"));
    assert!(!running.text.contains("i-07b9d1f3a5c8e2046"));
}

#[tokio::test]
async fn ec2_stop_and_tag_change_state() {
    let w = world("cloud/aws/stop-legacy-reporting-worker");
    let app = app(&w);
    let stop = query(
        &app,
        "ec2",
        "Action=StopInstances&Version=2016-11-15&InstanceId.1=i-07b9d1f3a5c8e2046",
    )
    .await;
    assert!(
        stop.text
            .contains("<currentState><code>64</code><name>stopping</name></currentState>")
    );
    query(
        &app,
        "ec2",
        "Action=CreateTags&Version=2016-11-15&ResourceId.1=i-07b9d1f3a5c8e2046&Tag.1.Key=DecommissionTicket&Tag.1.Value=OPS-4821",
    )
    .await;
    let s = state(&w);
    let inst = &s["instances"]["i-07b9d1f3a5c8e2046"];
    assert_eq!(inst["instanceState"]["name"], "stopped");
    assert!(
        inst["tagSet"]
            .as_array()
            .unwrap()
            .contains(&json!({"key": "DecommissionTicket", "value": "OPS-4821"}))
    );
    assert!(inst.get("publicIpAddress").is_none());
    assert_eq!(
        s["instances"]["i-0e4a6c8b2d1f35790"]["instanceState"]["name"],
        "running"
    );

    let start = query(
        &app,
        "ec2",
        "Action=StartInstances&Version=2016-11-15&InstanceId.1=i-07b9d1f3a5c8e2046",
    )
    .await;
    assert!(start.text.contains("<name>pending</name>"));
}

#[tokio::test]
async fn ec2_errors_and_dry_run() {
    let w = world("cloud/aws/stop-legacy-reporting-worker");
    let app = app(&w);
    let resp = query(
        &app,
        "ec2",
        "Action=StopInstances&Version=2016-11-15&InstanceId.1=i-0000000000000000a",
    )
    .await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    let x = resp.xml();
    let e = x.child("Errors").unwrap().child("Error").unwrap();
    assert_eq!(e.text_of("Code").unwrap(), "InvalidInstanceID.NotFound");
    let dry = query(
        &app,
        "ec2",
        "Action=TerminateInstances&Version=2016-11-15&InstanceId.1=i-07b9d1f3a5c8e2046&DryRun=true",
    )
    .await;
    assert_eq!(dry.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        state(&w)["instances"]["i-07b9d1f3a5c8e2046"]["instanceState"]["name"],
        "running"
    );
}

#[tokio::test]
async fn ec2_security_group_rules_round_trip() {
    let w = world("cloud/aws/stop-legacy-reporting-worker");
    let app = app(&w);
    let add = "Action=AuthorizeSecurityGroupIngress&Version=2016-11-15&GroupId=sg-0f2a4c6e8b1d35790&IpPermissions.1.IpProtocol=tcp&IpPermissions.1.FromPort=443&IpPermissions.1.ToPort=443&IpPermissions.1.IpRanges.1.CidrIp=10.30.0.0%2F16";
    assert_eq!(query(&app, "ec2", add).await.status, StatusCode::OK);
    let dup = query(&app, "ec2", add).await;
    assert!(dup.text.contains("InvalidPermission.Duplicate"));
    let rules = &state(&w)["security_groups"]["sg-0f2a4c6e8b1d35790"]["ipPermissions"];
    assert_eq!(rules[0]["fromPort"], 443);
    let revoke = add.replace(
        "AuthorizeSecurityGroupIngress",
        "RevokeSecurityGroupIngress",
    );
    assert_eq!(query(&app, "ec2", &revoke).await.status, StatusCode::OK);
    assert_eq!(
        state(&w)["security_groups"]["sg-0f2a4c6e8b1d35790"]["ipPermissions"],
        json!([])
    );
}

// ───────────────────────────── S3 ─────────────────────────────

#[tokio::test]
async fn s3_lists_buckets_and_reports_missing_encryption() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let list = rest(&app, "s3", "GET", "/", "").await;
    let x = list.xml();
    assert_eq!(x.child("Buckets").unwrap().all("Bucket").len(), 7);

    let none = rest(
        &app,
        "s3",
        "GET",
        "/northwind-legacy-exports?encryption",
        "",
    )
    .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
    assert!(
        none.text
            .contains("<Code>ServerSideEncryptionConfigurationNotFoundError</Code>")
    );
    let kms = rest(&app, "s3", "GET", "/northwind-data-lake-raw?encryption", "").await;
    assert!(kms.text.contains("<SSEAlgorithm>aws:kms</SSEAlgorithm>"));
    let default = rest(&app, "s3", "GET", "/northwind-app-assets?encryption", "").await;
    assert!(default.text.contains("<SSEAlgorithm>AES256</SSEAlgorithm>"));
    let missing = rest(&app, "s3", "GET", "/nope-bucket?encryption", "").await;
    assert!(missing.text.contains("<Code>NoSuchBucket</Code>"));
}

#[tokio::test]
async fn s3_encryption_and_versioning_updates_persist() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let body = "<ServerSideEncryptionConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Rule><ApplyServerSideEncryptionByDefault><SSEAlgorithm>aws:kms</SSEAlgorithm><KMSMasterKeyID>alias/finance</KMSMasterKeyID></ApplyServerSideEncryptionByDefault><BucketKeyEnabled>true</BucketKeyEnabled></Rule></ServerSideEncryptionConfiguration>";
    let put = rest(
        &app,
        "s3",
        "PUT",
        "/northwind-legacy-exports?encryption",
        body,
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    assert_eq!(
        state(&w)["buckets"]["northwind-legacy-exports"]["Encryption"],
        json!({"SSEAlgorithm": "aws:kms", "BucketKeyEnabled": true, "KMSMasterKeyID": "alias/finance"})
    );
    rest(
        &app,
        "s3",
        "PUT",
        "/northwind-legacy-exports?versioning",
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    let v = rest(
        &app,
        "s3",
        "GET",
        "/northwind-legacy-exports?versioning",
        "",
    )
    .await;
    assert!(v.text.contains("<Status>Enabled</Status>"));
}

#[tokio::test]
async fn s3_objects_list_with_delimiter_and_round_trip() {
    let w = world("cloud/aws/find-unencrypted-bucket");
    let app = app(&w);
    let list = rest(
        &app,
        "s3",
        "GET",
        "/northwind-legacy-exports?list-type=2&delimiter=%2F&prefix=",
        "",
    )
    .await;
    assert!(
        list.text
            .contains("<CommonPrefixes><Prefix>2017/</Prefix></CommonPrefixes>")
    );
    assert!(list.text.contains("<KeyCount>1</KeyCount>"));
    let paged = rest(
        &app,
        "s3",
        "GET",
        "/northwind-legacy-exports?list-type=2&max-keys=1",
        "",
    )
    .await;
    assert!(paged.text.contains("<IsTruncated>true</IsTruncated>"));
    assert!(
        paged
            .text
            .contains("<NextContinuationToken>2017/06/customers.csv</NextContinuationToken>")
    );

    let put = rest(
        &app,
        "s3",
        "PUT",
        "/northwind-app-assets/notes/readme.txt",
        "hello",
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    let got = rest(
        &app,
        "s3",
        "GET",
        "/northwind-app-assets/notes/readme.txt",
        "",
    )
    .await;
    assert_eq!(got.text, "hello");
    assert_eq!(got.headers["x-amz-server-side-encryption"], "AES256");
    let missing = rest(
        &app,
        "s3",
        "GET",
        "/northwind-app-assets/notes/nope.txt",
        "",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.text.contains("NoSuchKey"));

    let not_empty = rest(&app, "s3", "DELETE", "/northwind-legacy-exports", "").await;
    assert_eq!(not_empty.status, StatusCode::CONFLICT);
    let created = rest(&app, "s3", "PUT", "/northwind-scratch", "").await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(
        rest(&app, "s3", "DELETE", "/northwind-scratch", "")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(state(&w)["buckets"].get("northwind-scratch").is_none());
}

// ───────────────────────────── CloudWatch ─────────────────────────────

const CW: &str = "GraniteServiceVersion20100801";

#[tokio::test]
async fn cloudwatch_alarm_lifecycle() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let put = target(
        &app,
        "monitoring",
        &format!("{CW}.PutMetricAlarm"),
        json!({
            "AlarmName": "orders-api-high-cpu", "Namespace": "AWS/EC2", "MetricName": "CPUUtilization",
            "Dimensions": [{"Name": "InstanceId", "Value": "i-04c1a7e2b9d35f018"}],
            "Statistic": "Average", "Period": 300, "EvaluationPeriods": 3, "Threshold": 80.0,
            "ComparisonOperator": "GreaterThanThreshold", "AlarmActions": ["arn:aws:sns:us-east-1:481562039417:ops-pager"],
        }),
    )
    .await;
    assert_eq!(put.status, StatusCode::OK);
    let alarm = &state(&w)["alarms"]["orders-api-high-cpu"];
    assert_eq!(alarm["StateValue"], "INSUFFICIENT_DATA");
    assert_eq!(alarm["Threshold"], 80.0);

    let described = target(
        &app,
        "monitoring",
        &format!("{CW}.DescribeAlarms"),
        json!({"AlarmNamePrefix": "orders-"}),
    )
    .await
    .json();
    let names: Vec<&str> = described["MetricAlarms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["AlarmName"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "orders-api-high-cpu",
            "orders-api-status-check",
            "orders-worker-high-cpu"
        ]
    );
    assert!(described["MetricAlarms"][0]["StateUpdatedTimestamp"].is_number());

    let bad = target(
        &app,
        "monitoring",
        &format!("{CW}.PutMetricAlarm"),
        json!({"AlarmName": "x", "EvaluationPeriods": 1, "ComparisonOperator": "Bigger"}),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        bad.headers["x-amzn-query-error"],
        "InvalidParameterValue;Sender"
    );

    let history = target(
        &app,
        "monitoring",
        &format!("{CW}.DescribeAlarmHistory"),
        json!({"AlarmName": "orders-api-high-cpu"}),
    )
    .await
    .json();
    assert_eq!(
        history["AlarmHistoryItems"][0]["HistoryItemType"],
        "ConfigurationUpdate"
    );

    let gone = target(
        &app,
        "monitoring",
        &format!("{CW}.DeleteAlarms"),
        json!({"AlarmNames": ["no-such-alarm"]}),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cloudwatch_metric_data_statistics_and_insights() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let start = 1_773_468_000; // 2026-03-14T06:00:00Z
    let data = target(
        &app,
        "monitoring",
        &format!("{CW}.GetMetricData"),
        json!({
            "StartTime": start, "EndTime": start + 3600, "ScanBy": "TimestampAscending",
            "MetricDataQueries": [
                {"Id": "m1", "MetricStat": {"Metric": {"Namespace": "AWS/EC2", "MetricName": "CPUUtilization", "Dimensions": [{"Name": "InstanceId", "Value": "i-04c1a7e2b9d35f018"}]}, "Period": 1800, "Stat": "Maximum"}},
                {"Id": "e1", "Expression": "m1 / 100"},
            ],
        }),
    )
    .await
    .json();
    assert_eq!(
        data["MetricDataResults"][0]["Timestamps"],
        json!([start, start + 1800])
    );
    assert_eq!(data["MetricDataResults"][0]["Values"], json!([58.4, 74.8]));
    assert_eq!(
        data["MetricDataResults"][1]["Values"],
        json!([0.584, 0.748])
    );

    let insights = target(
        &app,
        "monitoring",
        &format!("{CW}.GetMetricData"),
        json!({
            "StartTime": start, "EndTime": start + 3600,
            "MetricDataQueries": [{"Id": "q", "Period": 3600, "Expression": "SELECT AVG(\"CPUUtilization\") FROM SCHEMA(\"AWS/EC2\", \"InstanceId\") GROUP BY \"InstanceId\" ORDER BY AVG() DESC LIMIT 2"}],
        }),
    )
    .await
    .json();
    let labels: Vec<&str> = insights["MetricDataResults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["Label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["i-04c1a7e2b9d35f018", "i-0d72f4a6c8e1b3059"]);

    let stats = target(
        &app,
        "monitoring",
        &format!("{CW}.GetMetricStatistics"),
        json!({"Namespace": "AWS/EC2", "MetricName": "CPUUtilization", "Dimensions": [{"Name": "InstanceId", "Value": "i-09e5b3d1f7a2c6480"}], "StartTime": start, "EndTime": start + 3600, "Period": 3600, "Statistics": ["SampleCount", "Average"]}),
    )
    .await
    .json();
    assert_eq!(stats["Datapoints"][0]["SampleCount"], 12.0);
    assert_eq!(stats["Datapoints"][0]["Unit"], "Percent");

    let metrics = target(
        &app,
        "monitoring",
        &format!("{CW}.ListMetrics"),
        json!({"Namespace": "AWS/EC2", "MetricName": "StatusCheckFailed"}),
    )
    .await
    .json();
    assert_eq!(metrics["Metrics"].as_array().unwrap().len(), 1);
}

// ───────────────────────────── Logs ─────────────────────────────

const LOGS: &str = "Logs_20140328";

#[tokio::test]
async fn logs_groups_streams_and_events() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let groups = target(
        &app,
        "logs",
        &format!("{LOGS}.DescribeLogGroups"),
        json!({"logGroupNamePrefix": "/ec2/orders-api"}),
    )
    .await
    .json();
    assert_eq!(groups["logGroups"].as_array().unwrap().len(), 2);
    assert_eq!(
        groups["logGroups"][0]["arn"],
        "arn:aws:logs:us-east-1:481562039417:log-group:/ec2/orders-api/prod:*"
    );
    assert_eq!(
        groups["logGroups"][0]["creationTime"],
        1_768_807_200_000_i64
    );

    let paged = target(
        &app,
        "logs",
        &format!("{LOGS}.DescribeLogGroups"),
        json!({"limit": 2}),
    )
    .await
    .json();
    assert_eq!(paged["nextToken"], "tok-2");

    let streams = target(
        &app,
        "logs",
        &format!("{LOGS}.DescribeLogStreams"),
        json!({"logGroupName": "/ec2/orders-api/prod"}),
    )
    .await
    .json();
    assert_eq!(
        streams["logStreams"][0]["logStreamName"],
        "i-04c1a7e2b9d35f018/app"
    );

    let filtered = target(
        &app,
        "logs",
        &format!("{LOGS}.FilterLogEvents"),
        json!({"logGroupName": "/ec2/orders-api/prod", "filterPattern": "{ $.level = \"ERROR\" }"}),
    )
    .await
    .json();
    assert_eq!(filtered["events"].as_array().unwrap().len(), 1);

    let events = target(&app, "logs", &format!("{LOGS}.GetLogEvents"), json!({"logGroupName": "/ec2/orders-api/prod", "logStreamName": "i-04c1a7e2b9d35f018/app", "limit": 2, "startFromHead": true})).await.json();
    assert_eq!(events["events"].as_array().unwrap().len(), 2);

    let missing = target(
        &app,
        "logs",
        &format!("{LOGS}.DescribeLogStreams"),
        json!({"logGroupName": "/nope"}),
    )
    .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.json()["__type"], "ResourceNotFoundException");
}

#[tokio::test]
async fn logs_retention_validation_and_update() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let bad = target(
        &app,
        "logs",
        &format!("{LOGS}.PutRetentionPolicy"),
        json!({"logGroupName": "/ec2/orders-api/prod", "retentionInDays": 45}),
    )
    .await;
    assert_eq!(bad.json()["__type"], "InvalidParameterException");
    let ok = target(
        &app,
        "logs",
        &format!("{LOGS}.PutRetentionPolicy"),
        json!({"logGroupName": "/ec2/orders-api/prod", "retentionInDays": 90}),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(
        state(&w)["log_groups"]["/ec2/orders-api/prod"]["retentionInDays"],
        90
    );
}

#[tokio::test]
async fn logs_insights_queries_complete_immediately() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let start = target(
        &app,
        "logs",
        &format!("{LOGS}.StartQuery"),
        json!({
            "logGroupNames": ["/ec2/orders-api/prod"], "startTime": 1_773_468_000, "endTime": 1_773_478_800,
            "queryString": "filter status >= 500 or latency_ms > 3000 | fields @timestamp, route, latency_ms | sort latency_ms desc",
        }),
    )
    .await
    .json();
    let id = start["queryId"].as_str().unwrap().to_owned();
    let results = target(
        &app,
        "logs",
        &format!("{LOGS}.GetQueryResults"),
        json!({"queryId": id}),
    )
    .await
    .json();
    assert_eq!(results["status"], "Complete");
    let rows = results["results"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][2], json!({"field": "latency_ms", "value": "30001"}));
    assert_eq!(rows[0][0]["value"], "2026-03-14 08:02:17.000");
    assert_eq!(results["statistics"]["recordsScanned"], 6.0);

    let malformed = target(&app, "logs", &format!("{LOGS}.StartQuery"), json!({"logGroupNames": ["/ec2/orders-api/prod"], "startTime": 1, "endTime": 2, "queryString": "explode now"})).await;
    assert_eq!(malformed.json()["__type"], "MalformedQueryException");
    let unknown = target(
        &app,
        "logs",
        &format!("{LOGS}.GetQueryResults"),
        json!({"queryId": "nope"}),
    )
    .await;
    assert_eq!(unknown.json()["__type"], "ResourceNotFoundException");
}

// ───────────────────────────── Lambda / Account ─────────────────────────────

#[tokio::test]
async fn lambda_functions_and_errors() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    w.lock().unwrap().service_mut("aws").store.collection_mut("functions").insert(
        "orders-export",
        json!({"FunctionName": "orders-export", "Runtime": "python3.12", "MemorySize": 256, "Timeout": 60, "LastModified": "2026-02-01T00:00:00Z", "Tags": {"team": "commerce"}}),
    );
    let app = app(&w);
    let list = rest(&app, "lambda", "GET", "/2015-03-31/functions/", "")
        .await
        .json();
    assert_eq!(
        list["Functions"][0]["FunctionArn"],
        "arn:aws:lambda:us-east-1:481562039417:function:orders-export"
    );
    assert_eq!(
        list["Functions"][0]["LastModified"],
        "2026-02-01T00:00:00.000+0000"
    );

    let got = rest(
        &app,
        "lambda",
        "GET",
        "/2015-03-31/functions/orders-export",
        "",
    )
    .await
    .json();
    assert_eq!(got["Tags"]["team"], "commerce");

    let missing = rest(&app, "lambda", "GET", "/2015-03-31/functions/ghost", "").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing.headers["x-amzn-errortype"],
        "ResourceNotFoundException"
    );

    let updated = rest(
        &app,
        "lambda",
        "PUT",
        "/2015-03-31/functions/orders-export/configuration",
        r#"{"MemorySize":1024}"#,
    )
    .await
    .json();
    assert_eq!(updated["MemorySize"], 1024);
    assert_eq!(state(&w)["functions"]["orders-export"]["MemorySize"], 1024);
}

#[tokio::test]
async fn account_lists_regions_for_region_expansion() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let regions = rest(&app, "account", "POST", "/listRegions", "{}")
        .await
        .json();
    let enabled = regions["Regions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["RegionOptStatus"] == "ENABLED_BY_DEFAULT")
        .count();
    assert_eq!(enabled, 17);
}

#[tokio::test]
async fn unmodelled_services_get_empty_protocol_correct_answers() {
    let w = world("cloud/aws/orders-api-cpu-alarm");
    let app = app(&w);
    let rds = query(&app, "rds", "Action=DescribeDBInstances&Version=2014-10-31").await;
    assert_eq!(rds.status, StatusCode::OK);
    assert!(rds.text.contains("<DescribeDBInstancesResult/>"));
    let ddb = target(&app, "dynamodb", "DynamoDB_20120810.ListTables", json!({})).await;
    assert_eq!(ddb.json(), json!({}));
}
