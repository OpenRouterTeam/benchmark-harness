//! `POST /api/graphql`: a GraphQL executor over the same store as the REST API.

mod exec;
mod mutate;
pub mod objects;
pub mod parse;
mod resolve;

use axum::{http::StatusCode, response::Response};
use mock_core::http::{GraphqlRequest, json};
use serde_json::{Value, json as json_value};

use crate::Ctx;

pub use resolve::Gh;

pub fn handle(ctx: &mut Ctx) -> Response {
    let Ok(req) = serde_json::from_slice::<GraphqlRequest>(&ctx.req.body) else {
        return json(
            StatusCode::BAD_REQUEST,
            json_value!({"message": "Problems parsing JSON", "documentation_url": "https://docs.github.com/graphql"}),
        );
    };
    let doc = match parse::parse(&req.query) {
        Ok(d) => d,
        Err(e) => {
            return json(
                StatusCode::OK,
                json_value!({"errors": [{"message": e, "locations": []}]}),
            );
        }
    };
    let viewer = ctx.viewer();
    let gh = Gh {
        svc: ctx.svc,
        base: ctx.req.base.clone(),
        now: ctx.now.clone(),
        viewer,
    };
    let mut exec = exec::Executor::new(&doc, req.variables, gh);
    let op = match exec.operation(req.operation_name.as_deref()) {
        Ok(op) => op,
        Err(e) => return json(StatusCode::OK, json_value!({"errors": [{"message": e}]})),
    };
    let data = exec.run(op);
    let mut body = json_value!({"data": data});
    if !exec.errors.is_empty() {
        body["errors"] = Value::Array(exec.errors);
    }
    json(StatusCode::OK, body)
}
