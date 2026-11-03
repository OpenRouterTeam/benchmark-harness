use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_stripe::Stripe;
use serde_json::{Value, json};
use tower::ServiceExt;

const ACCT: &str = "acct_1FwY8bQk2LmN4pRs";

fn world() -> SharedWorld {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap())
}

fn setup() -> (SharedWorld, Router) {
    let w = world();
    let app = service_app(w.clone(), &Stripe);
    (w, app)
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()["services"]["stripe"].clone()
}

/// Calls a replica tool; returns `(is_error, text)`.
async fn tool(app: &Router, name: &str, arguments: Value) -> (bool, String) {
    let req = Request::post("/_mcp/tools/call")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name": name, "arguments": arguments}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    (
        body["isError"] == true,
        body["content"][0]["text"].as_str().unwrap().to_owned(),
    )
}

async fn api(app: &Router, write: bool, op: &str, parameters: Value) -> Result<Value, String> {
    let name = if write {
        "stripe_api_write"
    } else {
        "stripe_api_read"
    };
    let (err, text) = tool(
        app,
        name,
        json!({"stripe_api_operation_id": op, "parameters": parameters, "stripe_context": ACCT, "livemode": false}),
    )
    .await;
    if err {
        Err(text)
    } else {
        Ok(serde_json::from_str(&text).unwrap())
    }
}

async fn read(app: &Router, op: &str, parameters: Value) -> Value {
    api(app, false, op, parameters)
        .await
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

async fn write(app: &Router, op: &str, parameters: Value) -> Value {
    api(app, true, op, parameters)
        .await
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

async fn rest(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer sk_test_toolbench")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn ids(list: &Value) -> Vec<String> {
    list["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect()
}

// --- session and discovery ----------------------------------------------------------

#[tokio::test]
async fn accounts_and_context_validation() {
    let (_, app) = setup();
    let (err, text) = tool(&app, "list_available_accounts_or_orgs", json!({})).await;
    assert!(!err);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v,
        json!({"accounts": [{"stripe_context": ACCT, "livemode": false, "name": "Fernway Software"}]})
    );

    let (err, text) = tool(
        &app,
        "stripe_api_read",
        json!({"stripe_api_operation_id": "GetCustomers", "parameters": {}, "stripe_context": "acct_other", "livemode": false}),
    )
    .await;
    assert!(err);
    assert!(text.starts_with("No account found for the provided stripe_context"));
    let (err, text) = tool(
        &app,
        "stripe_api_read",
        json!({"stripe_api_operation_id": "GetCustomers", "parameters": {}, "stripe_context": ACCT, "livemode": true}),
    )
    .await;
    assert!(err);
    assert_eq!(
        text,
        format!(
            "The provided account {ACCT} is a sandbox account. Retry with livemode set to false."
        )
    );
    let (err, text) = tool(&app, "manage_stripe_accounts", json!({})).await;
    assert!(!err);
    assert!(text.contains("https://access.stripe.com/mcp/oauth2/authorize/sessions/oases_"));
}

#[tokio::test]
async fn search_and_details_serve_the_catalogue() {
    let (_, app) = setup();
    let (err, text) = tool(
        &app,
        "stripe_api_search",
        json!({"intent": "create", "resource": "refund", "stripe_context": ACCT, "livemode": false}),
    )
    .await;
    assert!(!err);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["openapi_spec_version"], "2026-08-26.preview");
    assert_eq!(v["data"][0]["id"], "PostRefunds");
    assert_eq!(v["data"][0]["path"], "/v1/refunds");
    assert!(v["data"].as_array().unwrap().len() <= 5);

    let (err, text) = tool(
        &app,
        "stripe_api_details",
        json!({"stripe_api_operation_id": "PostSubscriptionsSubscriptionExposedId", "stripe_context": ACCT, "livemode": false}),
    )
    .await;
    assert!(!err);
    let d: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(d["path"], "/v1/subscriptions/{id}");
    assert_eq!(
        d["parameters"]["body"]["cancel_at_period_end"]["type"],
        "boolean"
    );
    assert_eq!(d["required_permissions"], json!(["subscription_write"]));

    for op in ["GetEvents", "PostCharges", "Nope"] {
        let (err, text) = tool(
            &app,
            "stripe_api_details",
            json!({"stripe_api_operation_id": op, "stripe_context": ACCT, "livemode": false}),
        )
        .await;
        assert!(err);
        assert_eq!(
            text,
            format!(
                "Operation '{op}' is not available. Use stripe_api_search to find available operations."
            )
        );
    }
}

#[tokio::test]
async fn operation_gate_and_parameter_errors() {
    let (_, app) = setup();
    // A write operation through the read tool, and an absent operation.
    let err = api(&app, false, "PostRefunds", json!({"charge": "ch_x"}))
        .await
        .unwrap_err();
    assert!(err.starts_with("Operation 'PostRefunds' is not available."));
    let err = api(&app, false, "GetEvents", json!({})).await.unwrap_err();
    assert!(err.starts_with("Operation 'GetEvents' is not available."));
    // Unknown parameter and missing resource, with the details hint.
    let err = api(&app, false, "GetCustomers", json!({"name": "x"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Stripe API error: Received unknown parameter: name\n\nUse stripe_api_details with stripe_api_operation_id: \"GetCustomers\" to see all required and optional parameters."
    );
    let err = api(
        &app,
        false,
        "GetCustomersCustomer",
        json!({"customer": "cus_nope"}),
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("Stripe API error: No such customer: 'cus_nope'"));
    let err = api(&app, false, "GetCustomersCustomer", json!({}))
        .await
        .unwrap_err();
    assert!(err.starts_with("Stripe API error: Missing required param: id."));
    // Products this account has not activated.
    let err = api(&app, false, "GetIssuingCards", json!({}))
        .await
        .unwrap_err();
    assert!(err.starts_with("Stripe API error: Your account is not set up to use Issuing."));
    let (err, text) = tool(
        &app,
        "stripe_analytics",
        json!({"intent": "execute_query_template", "stripe_context": ACCT, "livemode": false}),
    )
    .await;
    assert!(err);
    assert!(text.contains("not set up to use Sigma"));
}

// --- customers, lists, search, expand -------------------------------------------------

#[tokio::test]
async fn customers_list_paginates_newest_first_and_skips_deleted() {
    let (_, app) = setup();
    let all = read(&app, "GetCustomers", json!({})).await;
    assert_eq!(all["object"], "list");
    assert_eq!(all["url"], "/v1/customers");
    assert_eq!(all["has_more"], false);
    assert_eq!(
        ids(&all),
        [
            "cus_LumenData00003",
            "cus_Northwind00002",
            "cus_HarborPine0001"
        ]
    );
    let page1 = read(&app, "GetCustomers", json!({"limit": 2})).await;
    assert_eq!(ids(&page1), ["cus_LumenData00003", "cus_Northwind00002"]);
    assert_eq!(page1["has_more"], true);
    let page2 = read(
        &app,
        "GetCustomers",
        json!({"limit": 2, "starting_after": "cus_Northwind00002"}),
    )
    .await;
    assert_eq!(ids(&page2), ["cus_HarborPine0001"]);
    assert_eq!(page2["has_more"], false);
    let back = read(
        &app,
        "GetCustomers",
        json!({"limit": 1, "ending_before": "cus_HarborPine0001"}),
    )
    .await;
    assert_eq!(ids(&back), ["cus_Northwind00002"]);
    assert_eq!(back["has_more"], true);
    let err = api(
        &app,
        false,
        "GetCustomers",
        json!({"starting_after": "cus_gone"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("No such customer: 'cus_gone'"));
    let by_email = read(
        &app,
        "GetCustomers",
        json!({"email": "owner@northwindbakery.com"}),
    )
    .await;
    assert_eq!(ids(&by_email), ["cus_Northwind00002"]);

    // Rendering fills Stripe's defaults and nulls.
    let c = &all["data"][2];
    assert_eq!(c["object"], "customer");
    assert_eq!(c["livemode"], false);
    assert_eq!(c["balance"], 0);
    assert_eq!(c["address"], Value::Null);
    assert_eq!(c["tax_exempt"], "none");
    let deleted = read(
        &app,
        "GetCustomersCustomer",
        json!({"id": "cus_DeletedCo00004"}),
    )
    .await;
    assert_eq!(
        deleted,
        json!({"id": "cus_DeletedCo00004", "object": "customer", "deleted": true})
    );
}

#[tokio::test]
async fn search_language_and_expand() {
    let (_, app) = setup();
    let r = read(
        &app,
        "GetCustomersSearch",
        json!({"query": "name~\"harbor\" AND metadata[\"tier\"]:\"gold\""}),
    )
    .await;
    assert_eq!(r["object"], "search_result");
    assert_eq!(r["url"], "/v1/customers/search");
    assert_eq!(ids(&r), ["cus_HarborPine0001"]);
    assert_eq!(r["next_page"], Value::Null);
    let err = api(
        &app,
        false,
        "GetCustomersSearch",
        json!({"query": "amount>5"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unsupported search field"));
    let charges = read(
        &app,
        "GetChargesSearch",
        json!({"query": "customer:\"cus_Northwind00002\" AND amount>=40000", "expand": ["data.customer"]}),
    )
    .await;
    assert_eq!(ids(&charges), ["ch_NorthwindMarA00000000002"]);
    assert_eq!(charges["data"][0]["customer"]["name"], "Northwind Bakery");

    let sub = read(
        &app,
        "GetSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "expand": ["latest_invoice", "customer"]}),
    )
    .await;
    assert_eq!(sub["latest_invoice"]["number"], "HPC1F0A2-0012");
    assert_eq!(sub["customer"]["email"], "ap@harborpine.coffee");
    assert_eq!(sub["items"]["data"][0]["price"]["unit_amount"], 24900);
    assert_eq!(sub["items"]["data"][1]["quantity"], 6);
    let err = api(
        &app,
        false,
        "GetChargesCharge",
        json!({"id": "ch_NorthwindFeb000000000001", "expand": ["amount"]}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("This property cannot be expanded (amount)."));
    let err = api(&app, false, "GetCharges", json!({"expand": ["customer"]}))
        .await
        .unwrap_err();
    assert!(err.contains("You may want to try expanding 'data.customer' instead."));
    let charge = read(
        &app,
        "GetChargesCharge",
        json!({"id": "ch_LumenPartial00000000004", "expand": ["refunds"]}),
    )
    .await;
    assert_eq!(charge["refunds"]["object"], "list");
    assert!(charge.get("refunds").is_some());
    let plain = read(
        &app,
        "GetChargesCharge",
        json!({"id": "ch_LumenPartial00000000004"}),
    )
    .await;
    assert!(plain.get("refunds").is_none(), "refunds is expand-only");
}

#[tokio::test]
async fn customer_create_and_update_reflect_in_state() {
    let (w, app) = setup();
    let c = write(
        &app,
        "PostCustomers",
        json!({"name": "Ada Ops", "email": "ada@example.com", "metadata": {"source": "mcp"}}),
    )
    .await;
    let id = c["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("cus_") && id.len() == 18);
    // One tick per tool call: 2026-03-18T15:00:07Z.
    assert_eq!(c["created"], 1_773_846_007);
    let u = write(
        &app,
        "PostCustomersCustomer",
        json!({"id": id, "phone": "+15550100", "metadata": {"source": "", "tier": "bronze"}}),
    )
    .await;
    assert_eq!(u["metadata"], json!({"tier": "bronze"}));
    let s = state(&w);
    assert_eq!(s["customers"][&id]["phone"], "+15550100");
    assert_eq!(s["customers"][&id]["email"], "ada@example.com");
    let events: Vec<&str> = s["events"]
        .as_object()
        .unwrap()
        .values()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["customer.created", "customer.updated"]);
}

// --- refunds -----------------------------------------------------------------------

#[tokio::test]
async fn refunds_update_the_charge_and_balance() {
    let (w, app) = setup();
    let before = read(&app, "GetBalance", json!({})).await;
    assert_eq!(before["object"], "balance");
    let available_before = before["available"][0]["amount"].as_i64().unwrap();

    let refund = write(
        &app,
        "PostRefunds",
        json!({"charge": "ch_NorthwindMarA00000000002", "reason": "duplicate"}),
    )
    .await;
    assert_eq!(refund["object"], "refund");
    assert_eq!(refund["amount"], 48000);
    assert_eq!(refund["status"], "succeeded");
    assert_eq!(refund["reason"], "duplicate");
    assert_eq!(refund["payment_intent"], "pi_NorthwindMarA00000000002");
    let s = state(&w);
    let charge = &s["charges"]["ch_NorthwindMarA00000000002"];
    assert_eq!(charge["amount_refunded"], 48000);
    assert_eq!(charge["refunded"], true);
    let after = read(&app, "GetBalance", json!({})).await;
    assert_eq!(
        after["available"][0]["amount"].as_i64().unwrap(),
        available_before - 48000
    );
    let err = api(
        &app,
        true,
        "PostRefunds",
        json!({"charge": "ch_NorthwindMarA00000000002"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Charge ch_NorthwindMarA00000000002 has already been refunded."));

    // Partial refunds through the PaymentIntent, and the over-refund message.
    let err = api(
        &app,
        true,
        "PostRefunds",
        json!({"payment_intent": "pi_NorthwindFeb000000000001", "amount": 20000}),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains(
            "Refund amount ($200.00) is greater than unrefunded amount on charge ($120.00)"
        )
    );
    let part = write(
        &app,
        "PostRefunds",
        json!({"payment_intent": "pi_NorthwindFeb000000000001", "amount": "2000"}),
    )
    .await;
    assert_eq!(part["charge"], "ch_NorthwindFeb000000000001");
    let charge = read(
        &app,
        "GetChargesCharge",
        json!({"id": "ch_NorthwindFeb000000000001"}),
    )
    .await;
    assert_eq!(charge["amount_refunded"], 2000);
    assert_eq!(charge["refunded"], false);
    let listed = read(
        &app,
        "GetChargesChargeRefunds",
        json!({"id": "ch_NorthwindFeb000000000001"}),
    )
    .await;
    assert_eq!(ids(&listed), [part["id"].as_str().unwrap()]);

    let err = api(
        &app,
        true,
        "PostRefunds",
        json!({"charge": "ch_HarborDisputed0000000003"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("has been charged back; cannot issue a refund."));
    let err = api(&app, true, "PostRefunds", json!({"amount": 5}))
        .await
        .unwrap_err();
    assert!(err.contains("payment_intent or charge."));
    let err = api(
        &app,
        true,
        "PostRefunds",
        json!({"payment_intent": "pi_LumenPending000000000003"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("does not have a successful charge to refund."));
    let err = api(&app, true, "PostRefunds", json!({"charge": "ch_missing"}))
        .await
        .unwrap_err();
    assert!(err.contains("No such charge: 'ch_missing'"));
}

// --- subscriptions and discounts ------------------------------------------------------

#[tokio::test]
async fn cancel_at_period_end_then_immediately() {
    let (w, app) = setup();
    let sub = write(
        &app,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "cancel_at_period_end": true, "cancellation_details": {"comment": "Moving to annual"}}),
    )
    .await;
    assert_eq!(sub["status"], "active");
    assert_eq!(sub["cancel_at_period_end"], true);
    assert_eq!(sub["cancel_at"], 1_775_001_600);
    assert_eq!(
        sub["cancellation_details"]["reason"],
        "cancellation_requested"
    );
    assert_eq!(sub["cancellation_details"]["comment"], "Moving to annual");
    let s = state(&w);
    assert_eq!(
        s["subscriptions"]["sub_HarborPro000000000000001"]["cancel_at"],
        1_775_001_600
    );

    let undo = write(
        &app,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "cancel_at_period_end": false}),
    )
    .await;
    assert_eq!(undo["cancel_at"], Value::Null);
    assert_eq!(undo["canceled_at"], Value::Null);

    let gone = write(
        &app,
        "DeleteSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_LumenAnalytic0000000003"}),
    )
    .await;
    assert_eq!(gone["status"], "canceled");
    assert!(gone["ended_at"].is_i64());
    let err = api(
        &app,
        true,
        "DeleteSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_LumenAnalytic0000000003"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("No such subscription: 'sub_LumenAnalytic0000000003'"));
    let err = api(
        &app,
        true,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborOld000000000000002", "description": "x"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains(
        "A canceled subscription can only update its cancellation_details and metadata."
    ));

    // Default list hides canceled subscriptions; status=all shows them.
    let active = read(
        &app,
        "GetSubscriptions",
        json!({"customer": "cus_HarborPine0001"}),
    )
    .await;
    assert_eq!(ids(&active), ["sub_HarborPro000000000000001"]);
    let all = read(
        &app,
        "GetSubscriptions",
        json!({"customer": "cus_HarborPine0001", "status": "all"}),
    )
    .await;
    assert_eq!(all["data"].as_array().unwrap().len(), 2);
    let items = read(
        &app,
        "GetSubscriptionItems",
        json!({"subscription": "sub_HarborPro000000000000001"}),
    )
    .await;
    assert_eq!(ids(&items), ["si_HarborPro00001", "si_HarborSeats001"]);
}

#[tokio::test]
async fn discounts_apply_coupons_by_deterministic_key() {
    let (w, app) = setup();
    let err = api(
        &app,
        true,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "discounts": [{"coupon": "WINTER50"}]}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Coupon WINTER50 is expired and cannot be applied."));
    let err = api(
        &app,
        true,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "coupon": "RETAIN20-3M"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Received unknown parameter: coupon"));
    let sub = write(
        &app,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "discounts": [{"coupon": "RETAIN20-3M"}], "expand": ["discounts"]}),
    )
    .await;
    assert_eq!(sub["discounts"][0]["object"], "discount");
    assert_eq!(sub["discounts"][0]["source"]["coupon"]["id"], "RETAIN20-3M");
    let s = state(&w);
    let d = &s["discounts"]["sub_HarborPro000000000000001:RETAIN20-3M"];
    assert_eq!(d["subscription"], "sub_HarborPro000000000000001");
    assert_eq!(d["source"]["coupon"], "RETAIN20-3M");
    assert!(d["end"].is_i64());
    assert_eq!(s["coupons"]["RETAIN20-3M"]["times_redeemed"], 1);
    // Promotion codes resolve to their coupon; replacing drops the old discount.
    write(
        &app,
        "PostSubscriptionsSubscriptionExposedId",
        json!({"id": "sub_HarborPro000000000000001", "discounts": [{"promotion_code": "promo_Spring2026000000000001"}]}),
    )
    .await;
    let s = state(&w);
    assert!(
        s["discounts"]
            .get("sub_HarborPro000000000000001:RETAIN20-3M")
            .is_none()
    );
    assert_eq!(
        s["discounts"]["sub_HarborPro000000000000001:SPRING20"]["promotion_code"],
        "promo_Spring2026000000000001"
    );
}

#[tokio::test]
async fn subscription_create_bills_the_first_period() {
    let (w, app) = setup();
    let sub = write(
        &app,
        "PostSubscriptions",
        json!({"customer": "cus_HarborPine0001", "items": [{"price": "price_AnalyticsYear00000000001", "quantity": 1}], "expand": ["latest_invoice"]}),
    )
    .await;
    assert_eq!(sub["status"], "active");
    assert_eq!(sub["latest_invoice"]["status"], "paid");
    assert_eq!(sub["latest_invoice"]["amount_paid"], 120000);
    assert_eq!(sub["latest_invoice"]["number"], "HPC1F0A2-0013");
    assert_eq!(
        sub["items"]["data"][0]["current_period_end"]
            .as_i64()
            .unwrap()
            - sub["items"]["data"][0]["current_period_start"]
                .as_i64()
                .unwrap(),
        365 * 86_400
    );
    let s = state(&w);
    assert_eq!(
        s["customers"]["cus_HarborPine0001"]["next_invoice_sequence"],
        14
    );
    let charges = read(
        &app,
        "GetCharges",
        json!({"customer": "cus_HarborPine0001", "limit": 1}),
    )
    .await;
    assert_eq!(charges["data"][0]["amount"], 120000);
    assert_eq!(charges["data"][0]["description"], "Subscription creation");

    let err = api(
        &app,
        true,
        "PostSubscriptions",
        json!({"customer": "cus_HarborPine0001"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Missing required param: items."));
    let incomplete = write(
        &app,
        "PostSubscriptions",
        json!({"customer": "cus_LumenData00003", "items": [{"price": "price_ProMonthly00000000000001"}]}),
    )
    .await;
    assert_eq!(incomplete["status"], "incomplete");
}

// --- invoices ----------------------------------------------------------------------

#[tokio::test]
async fn invoice_lifecycle() {
    let (w, app) = setup();
    let draft = write(
        &app,
        "PostInvoices",
        json!({"customer": "cus_LumenData00003", "collection_method": "send_invoice", "days_until_due": 30, "pending_invoice_items_behavior": "include"}),
    )
    .await;
    let id = draft["id"].as_str().unwrap().to_owned();
    assert_eq!(draft["status"], "draft");
    assert_eq!(draft["amount_due"], 25000);
    let item = write(
        &app,
        "PostInvoiceitems",
        json!({"customer": "cus_LumenData00003", "invoice": id, "amount": 5000, "description": "Rush fee"}),
    )
    .await;
    assert_eq!(item["invoice"], id.as_str());
    let lines = read(&app, "GetInvoicesInvoiceLines", json!({"id": id})).await;
    assert_eq!(lines["data"].as_array().unwrap().len(), 2);
    let open = write(&app, "PostInvoicesInvoiceFinalize", json!({"id": id})).await;
    assert_eq!(open["status"], "open");
    assert_eq!(open["number"], "LUM0D3E4-0001");
    assert_eq!(open["amount_due"], 30000);
    assert!(open["hosted_invoice_url"].is_string());
    let err = api(&app, true, "PostInvoicesInvoiceFinalize", json!({"id": id}))
        .await
        .unwrap_err();
    assert!(err.contains("re-finalize"));
    let err = api(
        &app,
        true,
        "PostInvoiceitems",
        json!({"customer": "cus_LumenData00003", "invoice": id, "amount": 1}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("This invoice is no longer editable"));
    let void = write(&app, "PostInvoicesInvoiceVoid", json!({"id": id})).await;
    assert_eq!(void["status"], "void");
    let err = api(
        &app,
        true,
        "PostInvoicesInvoiceVoid",
        json!({"id": "in_HarborMar000000000000001"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("cannot be voided"));
    let uncollectible = write(
        &app,
        "PostInvoicesInvoiceMarkUncollectible",
        json!({"id": "in_LumenOpen00000000000002"}),
    )
    .await;
    assert_eq!(uncollectible["status"], "uncollectible");
    let s = state(&w);
    assert_eq!(s["invoices"][&id]["status"], "void");
    assert_eq!(
        s["invoiceitems"]["ii_LumenSetupFee0000000001"]["invoice"],
        id.as_str()
    );

    let open = read(&app, "GetInvoices", json!({"status": "paid"})).await;
    assert_eq!(ids(&open), ["in_HarborMar000000000000001"]);
    let by_sub = read(
        &app,
        "GetInvoicesSearch",
        json!({"query": "subscription:\"sub_HarborPro000000000000001\""}),
    )
    .await;
    assert_eq!(ids(&by_sub), ["in_HarborMar000000000000001"]);
    let preview = write(
        &app,
        "PostInvoicesCreatePreview",
        json!({"subscription": "sub_HarborPro000000000000001"}),
    )
    .await;
    assert_eq!(preview["total"], 24900 + 9000);
    assert_eq!(
        preview["lines"]["data"][0]["period"]["start"],
        1_775_001_600
    );
}

// --- catalog: products, prices, coupons, promotion codes ------------------------------

#[tokio::test]
async fn products_prices_coupons_and_promotion_codes() {
    let (w, app) = setup();
    let product = write(
        &app,
        "PostProducts",
        json!({"name": "Fernway Enterprise", "default_price_data": {"currency": "usd", "unit_amount": 99900, "recurring": {"interval": "month"}}}),
    )
    .await;
    assert!(product["id"].as_str().unwrap().starts_with("prod_"));
    let price_id = product["default_price"].as_str().unwrap().to_owned();
    let price = read(
        &app,
        "GetPricesPrice",
        json!({"id": price_id, "expand": ["product"]}),
    )
    .await;
    assert_eq!(price["type"], "recurring");
    assert_eq!(price["unit_amount_decimal"], "99900");
    assert_eq!(price["product"]["name"], "Fernway Enterprise");
    let monthly = read(
        &app,
        "GetPrices",
        json!({"product": "prod_FernwayPro0001", "recurring": {"interval": "month"}}),
    )
    .await;
    assert_eq!(ids(&monthly), ["price_ProMonthly00000000000001"]);
    let inactive = read(&app, "GetProducts", json!({"active": false})).await;
    assert_eq!(ids(&inactive), ["prod_Onboarding0001"]);
    let by_key = read(
        &app,
        "GetPricesSearch",
        json!({"query": "lookup_key:\"pro_yearly\""}),
    )
    .await;
    assert_eq!(ids(&by_key), ["price_ProYearly000000000000001"]);
    let err = api(
        &app,
        true,
        "PostProducts",
        json!({"description": "no name"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Missing required param: name."));

    let coupon = write(&app, "PostCoupons", json!({"percent_off": 15, "duration": "repeating", "duration_in_months": 6, "name": "Loyalty"})).await;
    let coupon_id = coupon["id"].as_str().unwrap().to_owned();
    assert_eq!(coupon_id.len(), 8);
    let err = api(&app, true, "PostCoupons", json!({"duration": "once"}))
        .await
        .unwrap_err();
    assert!(err.contains("Must provide percent_off or amount_off."));
    let promo = write(
        &app,
        "PostPromotionCodes",
        json!({"promotion": {"type": "coupon", "coupon": coupon_id}, "code": "LOYAL15"}),
    )
    .await;
    assert_eq!(promo["promotion"]["coupon"], coupon_id.as_str());
    let err = api(
        &app,
        true,
        "PostPromotionCodes",
        json!({"promotion": {"type": "coupon", "coupon": coupon_id}, "code": "loyal15"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("already exists"));
    let found = read(&app, "GetPromotionCodes", json!({"code": "loyal15"})).await;
    assert_eq!(ids(&found), [promo["id"].as_str().unwrap()]);
    let del = write(&app, "DeleteCouponsCoupon", json!({"id": coupon_id})).await;
    assert_eq!(
        del,
        json!({"id": coupon_id, "object": "coupon", "deleted": true})
    );
    assert!(state(&w)["coupons"].get(&coupon_id).is_none());
}

// --- payments: intents, disputes, payouts, links, checkout ---------------------------------

#[tokio::test]
async fn payments_disputes_payouts_links_and_checkout() {
    let (w, app) = setup();
    let pis = read(
        &app,
        "GetPaymentIntents",
        json!({"customer": "cus_Northwind00002"}),
    )
    .await;
    assert_eq!(
        ids(&pis),
        ["pi_NorthwindMarA00000000002", "pi_NorthwindFeb000000000001"]
    );
    assert_eq!(pis["data"][0]["amount_received"], 48000);
    let pending = read(
        &app,
        "GetPaymentIntentsSearch",
        json!({"query": "status:\"requires_payment_method\""}),
    )
    .await;
    assert_eq!(ids(&pending), ["pi_LumenPending000000000003"]);
    let pms = read(
        &app,
        "GetCustomersCustomerPaymentMethods",
        json!({"id": "cus_HarborPine0001"}),
    )
    .await;
    assert_eq!(pms["data"][0]["card"]["last4"], "4242");
    let pm = api(
        &app,
        false,
        "GetCustomersCustomerPaymentMethodsPaymentMethod",
        json!({"customer": "cus_Northwind00002", "payment_method": "pm_HarborVisa00000000000001"}),
    )
    .await
    .unwrap_err();
    assert!(pm.contains("No such PaymentMethod"));

    let dispute = write(
        &app,
        "PostDisputesDispute",
        json!({"id": "dp_HarborDispute0000000001", "evidence": {"uncategorized_text": "Machine was serviced on site."}}),
    )
    .await;
    assert_eq!(dispute["status"], "under_review");
    assert_eq!(
        dispute["evidence"]["uncategorized_text"],
        "Machine was serviced on site."
    );
    assert_eq!(dispute["evidence_details"]["submission_count"], 1);

    let payouts = read(&app, "GetPayouts", json!({"status": "in_transit"})).await;
    assert_eq!(ids(&payouts), ["po_Mar15000000000000000002"]);
    let txns = read(&app, "GetBalanceTransactions", json!({"type": "payout"})).await;
    assert_eq!(ids(&txns), ["txn_PayoutFeb0000000000003"]);
    assert_eq!(txns["data"][0]["net"], -11622);

    let link = write(
        &app,
        "PostPaymentLinks",
        json!({"line_items": [{"price": "price_ProMonthly00000000000001", "quantity": 2}]}),
    )
    .await;
    assert!(
        link["url"]
            .as_str()
            .unwrap()
            .starts_with("https://buy.stripe.com/test_")
    );
    assert!(link.get("_line_items").is_none());
    let items = read(
        &app,
        "GetPaymentLinksPaymentLinkLineItems",
        json!({"id": link["id"]}),
    )
    .await;
    assert_eq!(items["data"][0]["amount_total"], 49800);
    assert_eq!(items["data"][0]["description"], "Fernway Pro");
    let session = write(
        &app,
        "PostCheckoutSessions",
        json!({"mode": "subscription", "customer": "cus_Northwind00002", "line_items": [{"price": "price_ProYearly000000000000001", "quantity": 1}], "success_url": "https://fernway.io/thanks"}),
    )
    .await;
    assert_eq!(session["object"], "checkout.session");
    assert_eq!(session["amount_total"], 249000);
    assert_eq!(session["status"], "open");
    let err = api(
        &app,
        true,
        "PostCheckoutSessions",
        json!({"line_items": []}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("Missing required param: mode."));
    let s = state(&w);
    assert_eq!(
        s["disputes"]["dp_HarborDispute0000000001"]["status"],
        "under_review"
    );
    assert_eq!(s["checkout_sessions"].as_object().unwrap().len(), 1);
}

// --- the other tools ----------------------------------------------------------------

#[tokio::test]
async fn documentation_planner_and_feedback() {
    let (w, app) = setup();
    let (err, text) = tool(
        &app,
        "search_stripe_documentation",
        json!({"question": "How do I cancel a subscription at the end of the billing period?"}),
    )
    .await;
    assert!(!err);
    assert!(text.starts_with("## Cancel subscriptions"));
    let (err, text) = tool(
        &app,
        "stripe_implementation_planner",
        json!({"stripe_context": ACCT, "livemode": false, "message": "I sell a SaaS plan"}),
    )
    .await;
    assert!(!err);
    let guide: Value = serde_json::from_str(&text).unwrap();
    let guide_id = guide["guide_id"].as_str().unwrap().to_owned();
    let (_, text) = tool(
        &app,
        "stripe_implementation_planner",
        json!({"stripe_context": ACCT, "livemode": false, "guide_id": guide_id, "selected_leaf_nodes": ["billing_checkout"], "accept": true}),
    )
    .await;
    let plan: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(plan["status"], "accepted");
    assert_eq!(plan["plan"]["selected"], json!(["billing_checkout"]));
    let (err, text) = tool(
        &app,
        "send_stripe_mcp_feedback",
        json!({"sentiment": "positive", "quote": "search worked", "context": "refund flow", "source": "agent"}),
    )
    .await;
    assert!(!err, "{text}");
    let (err, _) = tool(
        &app,
        "send_stripe_mcp_feedback",
        json!({"sentiment": "great", "quote": "x", "context": "y", "source": "agent"}),
    )
    .await;
    assert!(err);
    assert_eq!(state(&w)["mcp_feedback"].as_object().unwrap().len(), 1);
}

// --- the REST face ------------------------------------------------------------------

#[tokio::test]
async fn rest_face_uses_form_encoding_and_real_errors() {
    let (w, app) = setup();
    let (status, body) = rest(&app, "GET", "/v1/customers/cus_nope", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({"error": {
            "code": "resource_missing",
            "doc_url": "https://stripe.com/docs/error-codes/resource-missing",
            "message": "No such customer: 'cus_nope'",
            "param": "id",
            "type": "invalid_request_error"
        }})
    );
    let (status, body) = rest(&app, "GET", "/v1/nothing", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Unrecognized request URL (GET: /v1/nothing).")
    );

    let (status, body) = rest(
        &app,
        "POST",
        "/v1/customers",
        "name=Grace+Hopper&email=grace%40example.com&metadata[team]=navy&expand[]=invoice_settings",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Grace Hopper");
    assert_eq!(body["metadata"], json!({"team": "navy"}));
    let id = body["id"].as_str().unwrap().to_owned();
    let (status, _) = rest(&app, "DELETE", &format!("/v1/customers/{id}"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state(&w)["customers"][&id]["deleted"], true);

    let (status, body) = rest(
        &app,
        "POST",
        "/v1/refunds",
        "charge=ch_NorthwindMarA00000000002&amount=1000",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["amount"], 1000);
    let (status, events) = rest(&app, "GET", "/v1/events?type=charge.*&limit=5", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events["data"][0]["type"], "charge.refunded");
    assert_eq!(
        events["data"][0]["data"]["previous_attributes"]["amount_refunded"],
        0
    );
    let (status, list) = rest(
        &app,
        "GET",
        "/v1/subscriptions?status=all&limit=1&expand[]=data.customer",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["data"][0]["customer"].is_object());

    let req = Request::get("/v1/customers").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn same_calls_same_bytes() {
    let run = || async {
        let (w, app) = setup();
        let mut out = Vec::new();
        out.push(tool(&app, "manage_stripe_accounts", json!({})).await.1);
        out.push(
            api(&app, true, "PostCustomers", json!({"name": "Det"}))
                .await
                .unwrap()
                .to_string(),
        );
        out.push(
            api(
                &app,
                true,
                "PostRefunds",
                json!({"charge": "ch_NorthwindFeb000000000001"}),
            )
            .await
            .unwrap()
            .to_string(),
        );
        out.push(state(&w).to_string());
        out
    };
    assert_eq!(run().await, run().await);
}
