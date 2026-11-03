//! Replays each `scenarios/business/stripe/*` oracle in-process and grades it: the oracle
//! must pass and doing nothing must fail (the same contract `deploy/oracle.sh` checks).

use std::{fs, path::Path};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use mock_core::{
    dataset::Dataset,
    grading::{Expect, GradeInput, grade},
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_stripe::Stripe;
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    expect: Expect,
    #[serde(default)]
    solution: Solution,
}

#[derive(Deserialize, Default)]
struct Solution {
    #[serde(default, rename = "call")]
    calls: Vec<Call>,
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Deserialize)]
struct Call {
    endpoint: String,
    tool: String,
    arguments: Value,
}

fn scenario_dirs() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/business/stripe");
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("scenario.toml").is_file())
        .collect();
    dirs.sort();
    dirs
}

async fn call(app: &Router, tool: &str, arguments: &Value) -> Value {
    let req = Request::post("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name": tool, "arguments": arguments}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn score(world: &SharedWorld, expect: &Expect, answer: Option<&str>) -> f64 {
    let w = world.lock().unwrap();
    let g = grade(&GradeInput {
        expect,
        state: w.snapshot(),
        journal: w.journal.entries(),
        answer,
        tools_ips: vec!["unknown".to_owned()],
        context_overflow: false,
    });
    g.reward
}

#[tokio::test]
async fn oracle_passes_and_nop_fails() {
    let dirs = scenario_dirs();
    assert_eq!(dirs.len(), 4, "four stripe scenarios");
    for dir in dirs {
        let file: File =
            toml::from_str(&fs::read_to_string(dir.join("scenario.toml")).unwrap()).unwrap();
        let fresh = || shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());

        let world = fresh();
        let app = service_app(world.clone(), &Stripe);
        for c in &file.solution.calls {
            assert_eq!(c.endpoint, "stripe");
            let result = call(&app, &c.tool, &c.arguments).await;
            assert_ne!(
                result["isError"],
                true,
                "{}: {} failed: {}",
                dir.display(),
                c.tool,
                result["content"][0]["text"]
            );
        }
        assert_eq!(
            score(&world, &file.expect, file.solution.answer.as_deref()),
            1.0,
            "{}: oracle",
            dir.display()
        );

        let idle = fresh();
        assert_eq!(
            score(&idle, &file.expect, None),
            0.0,
            "{}: nop",
            dir.display()
        );
    }
}

const ACCT: &str = "acct_1FwY8bQk2LmN4pRs";

/// Runs `writes` (operation id, parameters) through `stripe_api_write` and grades the result.
async fn attempt(slug: &str, writes: &[(&str, Value)], answer: Option<&str>) -> f64 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scenarios/business/stripe")
        .join(slug);
    let file: File =
        toml::from_str(&fs::read_to_string(dir.join("scenario.toml")).unwrap()).unwrap();
    let world = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let app = service_app(world.clone(), &Stripe);
    for (op, parameters) in writes {
        call(
            &app,
            "stripe_api_write",
            &json!({"stripe_api_operation_id": op, "parameters": parameters, "stripe_context": ACCT, "livemode": false}),
        )
        .await;
    }
    score(&world, &file.expect, answer)
}

#[tokio::test]
async fn alternative_correct_paths_pass_and_mistakes_fail() {
    // MRR: the right number in other spellings passes; common mistakes do not.
    for (answer, want) in [
        ("MRR is $439/month.", 1.0),
        ("439.00 USD", 1.0),
        ("$538.00 including the old Starter plan", 0.0),
        ("$1,439.00", 0.0),
        ("$691.50", 0.0),
    ] {
        assert_eq!(
            attempt("customer-mrr", &[], Some(answer)).await,
            want,
            "{answer}"
        );
    }
    assert_eq!(
        attempt(
            "customer-mrr",
            &[(
                "PostCustomersCustomer",
                json!({"id": "cus_mxxxdqMeoA1GEc", "description": "x"})
            )],
            Some("$439.00")
        )
        .await,
        0.0
    );

    // Refund: through the PaymentIntent is equally right; the original charge is not.
    let dup_pi = json!({"payment_intent": "pi_qZ8HsvmHI2loKwRfwA0eFYWI", "reason": "duplicate"});
    assert_eq!(
        attempt("refund-duplicate-charge", &[("PostRefunds", dup_pi)], None).await,
        1.0
    );
    let original = json!({"charge": "ch_ITHHURD4dPMFtMW9vBBsbgLF", "reason": "duplicate"});
    assert_eq!(
        attempt(
            "refund-duplicate-charge",
            &[("PostRefunds", original)],
            None
        )
        .await,
        0.0
    );
    let no_reason = json!({"charge": "ch_r1lxhPeTsZKQxIfuhPANytDa"});
    assert_eq!(
        attempt(
            "refund-duplicate-charge",
            &[("PostRefunds", no_reason)],
            None
        )
        .await,
        0.0
    );
    let partial =
        json!({"charge": "ch_r1lxhPeTsZKQxIfuhPANytDa", "reason": "duplicate", "amount": 24000});
    assert_eq!(
        attempt("refund-duplicate-charge", &[("PostRefunds", partial)], None).await,
        0.0
    );

    // Cancellation: an explicit cancel_at at the period end is equally right.
    let op = "PostSubscriptionsSubscriptionExposedId";
    let comment = json!({"comment": "Customer requested cancellation via email"});
    let at = json!({"id": "sub_eAeIb5HpilxLht73ad3sTPEH", "cancel_at": 1776018000, "cancellation_details": comment});
    assert_eq!(
        attempt("cancel-at-period-end", &[(op, at)], None).await,
        1.0
    );
    let now = json!({"id": "sub_eAeIb5HpilxLht73ad3sTPEH", "cancellation_details": comment});
    assert_eq!(
        attempt(
            "cancel-at-period-end",
            &[("DeleteSubscriptionsSubscriptionExposedId", now)],
            None
        )
        .await,
        0.0
    );
    let both = [
        (
            op,
            json!({"id": "sub_eAeIb5HpilxLht73ad3sTPEH", "cancel_at_period_end": true, "cancellation_details": comment}),
        ),
        (
            op,
            json!({"id": "sub_50927iiN3XYUjf7BdJEsSV7n", "cancel_at_period_end": true}),
        ),
    ];
    assert_eq!(attempt("cancel-at-period-end", &both, None).await, 0.0);

    // Coupon: right coupon on the wrong Maya, the wrong coupon, or an extra subscription fail;
    // correcting a wrong coupon to the right one passes.
    let wrong_maya =
        json!({"id": "sub_2BwOc3Og08AwUPGK5akzzYWH", "discounts": [{"coupon": "RETAIN20-3M"}]});
    assert_eq!(
        attempt("apply-retention-coupon", &[(op, wrong_maya)], None).await,
        0.0
    );
    let wrong_coupon =
        json!({"id": "sub_7fWxXdMb0rsO3dgqzvcplQuy", "discounts": [{"coupon": "RETAIN20-12M"}]});
    assert_eq!(
        attempt(
            "apply-retention-coupon",
            &[(op, wrong_coupon.clone())],
            None
        )
        .await,
        0.0
    );
    let storage_too =
        json!({"id": "sub_rgXMKqDnvuQz0FKgeUQUpWDn", "discounts": [{"coupon": "RETAIN20-3M"}]});
    let right =
        json!({"id": "sub_7fWxXdMb0rsO3dgqzvcplQuy", "discounts": [{"coupon": "RETAIN20-3M"}]});
    assert_eq!(
        attempt(
            "apply-retention-coupon",
            &[(op, right.clone()), (op, storage_too)],
            None
        )
        .await,
        0.0
    );
    assert_eq!(
        attempt(
            "apply-retention-coupon",
            &[(op, wrong_coupon), (op, right)],
            None
        )
        .await,
        1.0
    );
}
