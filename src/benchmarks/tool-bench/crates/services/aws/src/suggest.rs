//! `POST /suggest-aws-commands` (`ENDPOINT_SUGGEST_AWS_COMMANDS`): deterministic stand-in
//! for the hosted suggestion service. Ranks a fixed catalog of CLI commands by keyword
//! overlap with the query and returns up to 10 as `{"suggestions": [...]}`.

use axum::{body::Bytes, http::StatusCode, response::Response};
use serde_json::{Value, json};

use crate::util::json_response;

/// (command, description, required parameters, keywords)
const CATALOG: &[(&str, &str, &[&str], &str)] = &[
    (
        "aws ec2 describe-instances",
        "Describes the specified instances or all instances.",
        &[],
        "ec2 instance instances server servers vm list describe running stopped show find tag",
    ),
    (
        "aws ec2 start-instances",
        "Starts an Amazon EBS-backed instance that you've previously stopped.",
        &["--instance-ids"],
        "ec2 instance start boot resume",
    ),
    (
        "aws ec2 stop-instances",
        "Stops an Amazon EBS-backed instance.",
        &["--instance-ids"],
        "ec2 instance stop shutdown halt",
    ),
    (
        "aws ec2 terminate-instances",
        "Shuts down the specified instances. This operation is idempotent.",
        &["--instance-ids"],
        "ec2 instance terminate delete destroy remove",
    ),
    (
        "aws ec2 create-tags",
        "Adds or overwrites only the specified tags for the specified Amazon EC2 resource or resources.",
        &["--resources", "--tags"],
        "ec2 tag tags label add instance resource",
    ),
    (
        "aws ec2 describe-security-groups",
        "Describes the specified security groups or all of your security groups.",
        &[],
        "ec2 security group groups firewall rules sg ingress",
    ),
    (
        "aws ec2 authorize-security-group-ingress",
        "Adds the specified inbound (ingress) rules to a security group.",
        &["--group-id"],
        "ec2 security group ingress inbound allow rule port open add",
    ),
    (
        "aws ec2 revoke-security-group-ingress",
        "Removes the specified inbound (ingress) rules from a security group.",
        &["--group-id"],
        "ec2 security group ingress inbound revoke remove rule port close",
    ),
    (
        "aws ec2 describe-volumes",
        "Describes the specified EBS volumes or all of your EBS volumes.",
        &[],
        "ec2 ebs volume volumes disk storage",
    ),
    (
        "aws ec2 describe-vpcs",
        "Describes your VPCs.",
        &[],
        "ec2 vpc vpcs network",
    ),
    (
        "aws ec2 describe-subnets",
        "Describes your subnets.",
        &[],
        "ec2 subnet subnets network",
    ),
    (
        "aws ec2 describe-regions",
        "Describes the Regions that are enabled for your account.",
        &[],
        "ec2 region regions enabled",
    ),
    (
        "aws s3api list-buckets",
        "Returns a list of all buckets owned by the authenticated sender of the request.",
        &[],
        "s3 bucket buckets list storage all",
    ),
    (
        "aws s3 ls",
        "List S3 objects and common prefixes under a prefix or all S3 buckets.",
        &[],
        "s3 list ls objects files bucket contents",
    ),
    (
        "aws s3api get-bucket-encryption",
        "Returns the default encryption configuration for an Amazon S3 bucket.",
        &["--bucket"],
        "s3 bucket encryption encrypted unencrypted sse kms default",
    ),
    (
        "aws s3api put-bucket-encryption",
        "Configures default encryption and Amazon S3 Bucket Keys for an existing bucket.",
        &["--bucket", "--server-side-encryption-configuration"],
        "s3 bucket encryption enable encrypt sse kms set",
    ),
    (
        "aws s3api get-bucket-versioning",
        "Returns the versioning state of a bucket.",
        &["--bucket"],
        "s3 bucket versioning version",
    ),
    (
        "aws s3api put-bucket-versioning",
        "Sets the versioning state of an existing bucket.",
        &["--bucket", "--versioning-configuration"],
        "s3 bucket versioning enable version",
    ),
    (
        "aws s3api get-public-access-block",
        "Retrieves the PublicAccessBlock configuration for an Amazon S3 bucket.",
        &["--bucket"],
        "s3 bucket public access block exposure",
    ),
    (
        "aws s3api put-public-access-block",
        "Creates or modifies the PublicAccessBlock configuration for an Amazon S3 bucket.",
        &["--bucket", "--public-access-block-configuration"],
        "s3 bucket public access block enable",
    ),
    (
        "aws s3api get-bucket-policy",
        "Returns the policy of a specified bucket.",
        &["--bucket"],
        "s3 bucket policy permissions",
    ),
    (
        "aws s3api list-objects-v2",
        "Returns some or all (up to 1,000) of the objects in a bucket.",
        &["--bucket"],
        "s3 objects list files keys bucket size",
    ),
    (
        "aws s3api get-bucket-tagging",
        "Returns the tag set associated with the bucket.",
        &["--bucket"],
        "s3 bucket tags tagging",
    ),
    (
        "aws iam list-users",
        "Lists the IAM users that have the specified path prefix.",
        &[],
        "iam user users list people accounts",
    ),
    (
        "aws iam get-user",
        "Retrieves information about the specified IAM user.",
        &[],
        "iam user details",
    ),
    (
        "aws iam list-attached-user-policies",
        "Lists all managed policies that are attached to the specified IAM user.",
        &["--user-name"],
        "iam user policy policies attached permissions",
    ),
    (
        "aws iam attach-user-policy",
        "Attaches the specified managed policy to the specified user.",
        &["--user-name", "--policy-arn"],
        "iam user policy attach grant permission add",
    ),
    (
        "aws iam detach-user-policy",
        "Removes the specified managed policy from the specified user.",
        &["--user-name", "--policy-arn"],
        "iam user policy detach remove revoke",
    ),
    (
        "aws iam list-access-keys",
        "Returns information about the access key IDs associated with the specified IAM user.",
        &[],
        "iam access key keys credentials user",
    ),
    (
        "aws iam update-access-key",
        "Changes the status of the specified access key from Active to Inactive, or vice versa.",
        &["--access-key-id", "--status"],
        "iam access key deactivate disable inactive rotate",
    ),
    (
        "aws iam list-roles",
        "Lists the IAM roles that have the specified path prefix.",
        &[],
        "iam role roles list",
    ),
    (
        "aws iam list-policies",
        "Lists all the managed policies that are available in your AWS account.",
        &[],
        "iam policy policies managed list administratoraccess",
    ),
    (
        "aws iam list-groups-for-user",
        "Lists the IAM groups that the specified IAM user belongs to.",
        &["--user-name"],
        "iam group groups user membership",
    ),
    (
        "aws iam add-user-to-group",
        "Adds the specified user to the specified group.",
        &["--group-name", "--user-name"],
        "iam group user add member",
    ),
    (
        "aws sts get-caller-identity",
        "Returns details about the IAM user or role whose credentials are used to call the operation.",
        &[],
        "sts whoami identity account caller current",
    ),
    (
        "aws cloudwatch describe-alarms",
        "Retrieves the specified alarms.",
        &[],
        "cloudwatch alarm alarms alert alerts state firing",
    ),
    (
        "aws cloudwatch put-metric-alarm",
        "Creates or updates an alarm and associates it with the specified metric.",
        &[
            "--alarm-name",
            "--evaluation-periods",
            "--comparison-operator",
        ],
        "cloudwatch alarm create alert cpu utilization threshold metric configure high",
    ),
    (
        "aws cloudwatch delete-alarms",
        "Deletes the specified alarms.",
        &["--alarm-names"],
        "cloudwatch alarm delete remove",
    ),
    (
        "aws cloudwatch get-metric-statistics",
        "Gets statistics for the specified metric.",
        &[
            "--namespace",
            "--metric-name",
            "--start-time",
            "--end-time",
            "--period",
        ],
        "cloudwatch metric statistics cpu utilization average datapoints",
    ),
    (
        "aws cloudwatch list-metrics",
        "List the specified metrics.",
        &[],
        "cloudwatch metric metrics list",
    ),
    (
        "aws logs describe-log-groups",
        "Lists the specified log groups.",
        &[],
        "logs log group groups cloudwatch list retention",
    ),
    (
        "aws logs put-retention-policy",
        "Sets the retention of the specified log group.",
        &["--log-group-name", "--retention-in-days"],
        "logs log group retention days expire set",
    ),
    (
        "aws logs filter-log-events",
        "Lists log events from the specified log group.",
        &[],
        "logs log events search filter error errors grep",
    ),
    (
        "aws logs start-query",
        "Starts a query of one or more log groups using CloudWatch Logs Insights.",
        &["--start-time", "--end-time", "--query-string"],
        "logs insights query search analyze",
    ),
    (
        "aws lambda list-functions",
        "Returns a list of Lambda functions.",
        &[],
        "lambda function functions serverless list",
    ),
    (
        "aws lambda get-function-configuration",
        "Returns the version-specific settings of a Lambda function or version.",
        &["--function-name"],
        "lambda function configuration memory timeout settings",
    ),
    (
        "aws lambda update-function-configuration",
        "Modify the version-specific settings of a Lambda function.",
        &["--function-name"],
        "lambda function memory timeout update configuration environment",
    ),
    (
        "aws lambda invoke",
        "Invokes a Lambda function.",
        &["--function-name"],
        "lambda function invoke run execute call",
    ),
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_owned)
        .collect()
}

pub fn suggestions(query: &str) -> Value {
    let q = words(query);
    let mut scored: Vec<(usize, f64, Value)> = CATALOG
        .iter()
        .enumerate()
        .filter_map(|(i, (cmd, desc, params, keywords))| {
            let kw = words(keywords);
            let cmd_words = words(cmd);
            let hits = q
                .iter()
                .filter(|w| kw.contains(w) || cmd_words.contains(w))
                .count();
            if hits == 0 {
                return None;
            }
            let score = hits as f64 / q.len().max(1) as f64;
            let confidence = (score.min(1.0) * 100.0).round() / 100.0;
            Some((
                i,
                confidence,
                json!({
                    "command": cmd,
                    "confidence": confidence,
                    "description": desc,
                    "required_parameters": params,
                }),
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).expect("finite").then(a.0.cmp(&b.0)));
    json!({"suggestions": scored.into_iter().take(10).map(|(_, _, v)| v).collect::<Vec<_>>()})
}

pub fn serve(body: &Bytes) -> Response {
    let query = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("query").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    if query.trim().is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            "application/json",
            &json!({"message": "query must not be empty"}),
        );
    }
    json_response(StatusCode::OK, "application/json", &suggestions(&query))
}
