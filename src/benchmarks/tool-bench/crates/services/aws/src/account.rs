//! AWS Account Management (REST-JSON). `ListRegions` backs aws-api's `--region *` expansion.

use axum::{http::StatusCode, response::Response};
use serde_json::{Value, json};

use crate::{Ctx, Req, ec2::REGIONS, util::json_response};

const OPT_IN: [&str; 4] = ["af-south-1", "ap-east-1", "eu-south-1", "me-south-1"];

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let path = req.path.trim_end_matches('/');
    let body = match path {
        "/listRegions" => {
            let input = req.body_json();
            let filter: Vec<String> = input
                .get("RegionOptStatusContains")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let mut regions: Vec<Value> = REGIONS
                .iter()
                .map(|r| json!({"RegionName": r, "RegionOptStatus": "ENABLED_BY_DEFAULT"}))
                .chain(
                    OPT_IN
                        .iter()
                        .map(|r| json!({"RegionName": r, "RegionOptStatus": "DISABLED"})),
                )
                .filter(|r| {
                    filter.is_empty() || filter.iter().any(|f| r["RegionOptStatus"] == f.as_str())
                })
                .collect();
            regions.sort_by_key(|r| r["RegionName"].as_str().unwrap_or_default().to_owned());
            json!({"Regions": regions})
        }
        "/getRegionOptStatus" => {
            let name = req.body_json()["RegionName"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let status = if OPT_IN.contains(&name.as_str()) {
                "DISABLED"
            } else {
                "ENABLED_BY_DEFAULT"
            };
            json!({"RegionName": name, "RegionOptStatus": status})
        }
        "/getContactInformation" => {
            let alias = ctx
                .docs("account")
                .first()
                .and_then(|(_, a)| a.get("Alias").cloned())
                .unwrap_or(json!("toolbench"));
            json!({"ContactInformation": {"FullName": "Cloud Platform Team", "CompanyName": alias, "AddressLine1": "1 Main St", "City": "Seattle", "CountryCode": "US", "PostalCode": "98101", "PhoneNumber": "+12065550100"}})
        }
        _ => json!({}),
    };
    json_response(StatusCode::OK, "application/json", &body)
}
