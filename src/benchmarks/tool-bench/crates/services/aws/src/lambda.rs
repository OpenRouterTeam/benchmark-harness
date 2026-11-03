//! Lambda (REST-JSON). `functions` keyed by `FunctionName`: configuration fields as
//! GetFunctionConfiguration returns them, plus `Tags` (`{k: v}`) and an optional
//! `InvokeResponse` (the payload a synchronous Invoke returns).

use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{
    Ctx, Req,
    util::{json_response, respond, s, value_time, with_header},
};

const CT: &str = "application/json";

fn error(status: StatusCode, code: &str, msg: &str) -> Response {
    with_header(
        json_response(status, CT, &json!({"Type": "User", "message": msg})),
        "x-amzn-errortype",
        code,
    )
}

fn arn(ctx: &Ctx, name: &str) -> String {
    format!(
        "arn:aws:lambda:{}:{}:function:{name}",
        ctx.region, ctx.account
    )
}

fn lambda_time(secs: i64) -> String {
    let iso = crate::util::iso(secs);
    format!("{}.000+0000", iso.trim_end_matches('Z'))
}

fn config(ctx: &Ctx, f: &Value) -> Value {
    let name = s(f, "FunctionName");
    let mut out = Map::new();
    out.insert("FunctionName".into(), json!(name));
    out.insert("FunctionArn".into(), json!(arn(ctx, &name)));
    for (k, v) in f.as_object().cloned().unwrap_or_default() {
        if k.starts_with('_') || k == "Tags" || k == "InvokeResponse" || k == "FunctionName" {
            continue;
        }
        if k == "LastModified" {
            out.insert(
                k,
                json!(value_time(&v).map(lambda_time).unwrap_or_default()),
            );
        } else {
            out.insert(k, v);
        }
    }
    let defaults = [
        ("Runtime", json!("python3.12")),
        ("Handler", json!("app.handler")),
        ("CodeSize", json!(4096)),
        ("Description", json!("")),
        ("Timeout", json!(3)),
        ("MemorySize", json!(128)),
        (
            "CodeSha256",
            json!("x1hKaTu0rGw0Zt1nI7vp4wPUGHFGmlZj3gNHrJGfXt4="),
        ),
        ("Version", json!("$LATEST")),
        ("TracingConfig", json!({"Mode": "PassThrough"})),
        ("RevisionId", json!("4b3f6f2e-5c1d-4d0e-9d55-3a1f0f1e7a10")),
        ("State", json!("Active")),
        ("LastUpdateStatus", json!("Successful")),
        ("PackageType", json!("Zip")),
        ("Architectures", json!(["x86_64"])),
        ("EphemeralStorage", json!({"Size": 512})),
        (
            "SnapStart",
            json!({"ApplyOn": "None", "OptimizationStatus": "Off"}),
        ),
        (
            "LoggingConfig",
            json!({"LogFormat": "Text", "LogGroup": format!("/aws/lambda/{name}")}),
        ),
    ];
    for (k, v) in defaults {
        out.entry(k).or_insert(v);
    }
    out.entry("Role").or_insert_with(|| {
        json!(format!(
            "arn:aws:iam::{}:role/service-role/{name}-role",
            ctx.account
        ))
    });
    Value::Object(out)
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let parts: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    let not_found_fn = |ctx: &Ctx, name: &str| {
        error(
            StatusCode::NOT_FOUND,
            "ResourceNotFoundException",
            &format!("Function not found: {}", arn(ctx, name)),
        )
    };
    let lookup = |ctx: &Ctx, ident: &str| -> Option<(String, Value)> {
        let name = ident.rsplit(':').next().unwrap_or(ident);
        let name = if ident.contains(":function:") {
            ident
                .split(":function:")
                .nth(1)
                .unwrap_or(name)
                .split(':')
                .next()
                .unwrap_or(name)
        } else {
            name
        };
        ctx.get("functions", name)
            .filter(|f| ctx.in_region(f))
            .map(|f| (name.to_owned(), f))
    };
    match (&req.method, parts.as_slice()) {
        (&Method::GET, [_, "functions"]) => {
            let mut fns: Vec<Value> = ctx
                .docs("functions")
                .into_iter()
                .filter(|(_, f)| ctx.in_region(f))
                .map(|(_, f)| config(ctx, &f))
                .collect();
            fns.sort_by_key(|f| s(f, "FunctionName"));
            let max: usize = req
                .q("MaxItems")
                .and_then(|m| m.parse().ok())
                .unwrap_or(50)
                .clamp(1, 50);
            let start: usize = req.q("Marker").and_then(|m| m.parse().ok()).unwrap_or(0);
            let page: Vec<Value> = fns.iter().skip(start).take(max).cloned().collect();
            let mut out = json!({"Functions": page});
            if start + max < fns.len() {
                out["NextMarker"] = json!((start + max).to_string());
            }
            json_response(StatusCode::OK, CT, &out)
        }
        (&Method::GET, [_, "functions", ident]) => match lookup(ctx, ident) {
            Some((name, f)) => {
                let mut out = json!({
                    "Configuration": config(ctx, &f),
                    "Code": {"RepositoryType": "S3", "Location": format!("https://awslambda-{}-tasks.s3.{}.amazonaws.com/snapshots/{}/{name}", ctx.region, ctx.region, ctx.account)},
                });
                if let Some(t) = f.get("Tags").filter(|t| t.is_object()) {
                    out["Tags"] = t.clone();
                }
                if let Some(c) = f.get("ReservedConcurrentExecutions") {
                    out["Concurrency"] = json!({"ReservedConcurrentExecutions": c});
                }
                json_response(StatusCode::OK, CT, &out)
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::DELETE, [_, "functions", ident]) => match lookup(ctx, ident) {
            Some((name, _)) => {
                ctx.remove("functions", &name);
                respond(StatusCode::NO_CONTENT, CT, "")
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::GET, [_, "functions", ident, "configuration"]) => match lookup(ctx, ident) {
            Some((_, f)) => json_response(StatusCode::OK, CT, &config(ctx, &f)),
            None => not_found_fn(ctx, ident),
        },
        (&Method::PUT, [_, "functions", ident, "configuration"]) => match lookup(ctx, ident) {
            Some((name, mut f)) => {
                let input = req.body_json();
                if let Some(m) = input.get("MemorySize").and_then(Value::as_i64)
                    && !(128..=10240).contains(&m)
                {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "InvalidParameterValueException",
                        &format!(
                            "1 validation error detected: Value '{m}' at 'memorySize' failed to satisfy constraint: Member must have value less than or equal to 10240"
                        ),
                    );
                }
                if let Some(t) = input.get("Timeout").and_then(Value::as_i64)
                    && !(1..=900).contains(&t)
                {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "InvalidParameterValueException",
                        &format!(
                            "1 validation error detected: Value '{t}' at 'timeout' failed to satisfy constraint: Member must have value less than or equal to 900"
                        ),
                    );
                }
                for (k, v) in input.as_object().cloned().unwrap_or_default() {
                    if ["RevisionId", "FunctionName"].contains(&k.as_str()) {
                        continue;
                    }
                    f[k] = v;
                }
                f["LastModified"] = json!(crate::util::iso(ctx.now));
                let rev = ctx.svc.ids.next_uuid("lambda-revision").to_string();
                f["RevisionId"] = json!(rev);
                ctx.put("functions", &name, f.clone());
                json_response(StatusCode::OK, CT, &config(ctx, &f))
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::POST, [_, "functions", ident, "invocations"]) => match lookup(ctx, ident) {
            Some((_, f)) => {
                if req.header("x-amz-invocation-type") == Some("Event") {
                    return respond(StatusCode::ACCEPTED, CT, "");
                }
                let payload = f
                    .get("InvokeResponse")
                    .cloned()
                    .unwrap_or(json!({"statusCode": 200, "body": "{\"ok\":true}"}));
                with_header(
                    json_response(StatusCode::OK, CT, &payload),
                    "x-amz-executed-version",
                    "$LATEST",
                )
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::GET, [_, "functions", ident, "versions"]) => match lookup(ctx, ident) {
            Some((_, f)) => {
                json_response(StatusCode::OK, CT, &json!({"Versions": [config(ctx, &f)]}))
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::GET, [_, "functions", ident, "aliases"]) => match lookup(ctx, ident) {
            Some(_) => json_response(StatusCode::OK, CT, &json!({"Aliases": []})),
            None => not_found_fn(ctx, ident),
        },
        (&Method::GET, [_, "functions", ident, "concurrency"]) => match lookup(ctx, ident) {
            Some((_, f)) => {
                let out = match f.get("ReservedConcurrentExecutions") {
                    Some(c) => json!({"ReservedConcurrentExecutions": c}),
                    None => json!({}),
                };
                json_response(StatusCode::OK, CT, &out)
            }
            None => not_found_fn(ctx, ident),
        },
        (&Method::PUT, [_, "functions", ident, "concurrency"]) => match lookup(ctx, ident) {
            Some((name, mut f)) => {
                let c = req.body_json()["ReservedConcurrentExecutions"].clone();
                f["ReservedConcurrentExecutions"] = c.clone();
                ctx.put("functions", &name, f);
                json_response(
                    StatusCode::OK,
                    CT,
                    &json!({"ReservedConcurrentExecutions": c}),
                )
            }
            None => not_found_fn(ctx, ident),
        },
        (
            &Method::GET,
            [
                _,
                "functions",
                ident,
                "policy" | "url" | "event-invoke-config" | "code-signing-config",
            ],
        ) => match lookup(ctx, ident) {
            Some(_) => error(
                StatusCode::NOT_FOUND,
                "ResourceNotFoundException",
                "The resource you requested does not exist.",
            ),
            None => not_found_fn(ctx, ident),
        },
        (_, [_, "tags", rest @ ..]) => {
            let ident = rest.join("/");
            let Some((name, mut f)) = lookup(ctx, &ident) else {
                return not_found_fn(ctx, &ident);
            };
            match req.method {
                Method::GET => json_response(
                    StatusCode::OK,
                    CT,
                    &json!({"Tags": f.get("Tags").cloned().unwrap_or(json!({}))}),
                ),
                Method::POST => {
                    let mut tags = f
                        .get("Tags")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    for (k, v) in req.body_json()["Tags"]
                        .as_object()
                        .cloned()
                        .unwrap_or_default()
                    {
                        tags.insert(k, v);
                    }
                    f["Tags"] = Value::Object(tags);
                    ctx.put("functions", &name, f);
                    respond(StatusCode::NO_CONTENT, CT, "")
                }
                _ => {
                    let keys: Vec<String> = req
                        .query
                        .iter()
                        .filter(|(k, _)| k == "tagKeys")
                        .map(|(_, v)| v.clone())
                        .collect();
                    if let Some(t) = f.get_mut("Tags").and_then(Value::as_object_mut) {
                        for k in keys {
                            t.shift_remove(&k);
                        }
                    }
                    ctx.put("functions", &name, f);
                    respond(StatusCode::NO_CONTENT, CT, "")
                }
            }
        }
        (&Method::GET, [_, "event-source-mappings"]) => {
            json_response(StatusCode::OK, CT, &json!({"EventSourceMappings": []}))
        }
        (&Method::GET, [_, "layers"]) => json_response(StatusCode::OK, CT, &json!({"Layers": []})),
        (&Method::GET, [_, "account-settings"]) => {
            let fns = ctx.docs("functions");
            let size: i64 = fns
                .iter()
                .map(|(_, f)| f.get("CodeSize").and_then(Value::as_i64).unwrap_or(4096))
                .sum();
            json_response(
                StatusCode::OK,
                CT,
                &json!({
                    "AccountLimit": {"TotalCodeSize": 80_530_636_800_i64, "CodeSizeUnzipped": 262_144_000, "CodeSizeZipped": 52_428_800, "ConcurrentExecutions": 1000, "UnreservedConcurrentExecutions": 1000},
                    "AccountUsage": {"TotalCodeSize": size, "FunctionCount": fns.len()},
                }),
            )
        }
        _ => error(
            StatusCode::NOT_FOUND,
            "ResourceNotFoundException",
            "The resource you requested does not exist.",
        ),
    }
}
