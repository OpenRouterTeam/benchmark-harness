//! The ten tools of the remote Stripe MCP (`servers/snapshots/stripe.json`).
//!
//! `stripe_api_read`/`stripe_api_write` dispatch an operation id through the same handlers as
//! the REST face; success returns the bare Stripe object as JSON text, failure a tool error
//! `Stripe API error: <message>` with the server's `stripe_api_details` hint, as the remote
//! server renders them (wording per Kiln-AI/stripe_world's live probes).

use mock_core::mcp;
use serde_json::{Value, json};

use crate::{
    api, catalog, commerce,
    engine::Ctx,
    error::StripeError,
    params::{Params, as_bool, as_int},
};

const RECONSENT_URL: &str = "https://access.stripe.com/mcp/oauth2/authorize/sessions/";

pub fn call(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    let args = if args.is_object() {
        args.clone()
    } else {
        json!({})
    };
    let result = match name {
        "list_available_accounts_or_orgs" => Ok(list_accounts(ctx)),
        "manage_stripe_accounts" => Ok(manage_accounts(ctx)),
        "stripe_api_search" => api_search(ctx, &args),
        "stripe_api_details" => api_details(ctx, &args),
        "stripe_api_read" => api_call(ctx, &args, false),
        "stripe_api_write" => api_call(ctx, &args, true),
        "stripe_analytics" => analytics(ctx, &args),
        "search_stripe_documentation" => search_docs(&args),
        "stripe_implementation_planner" => planner(ctx, &args),
        "send_stripe_mcp_feedback" => feedback(ctx, &args),
        other => Err(format!("Unknown tool: {other}")),
    };
    match result {
        Ok(v) => v,
        Err(message) => mcp::error(message),
    }
}

fn compact(value: &Value) -> Value {
    mcp::text(serde_json::to_string(value).expect("JSON serializes"))
}

fn required<'a>(tool: &str, args: &'a Value, field: &str) -> Result<&'a Value, String> {
    args.get(field).filter(|v| !v.is_null()).ok_or_else(|| {
        format!("Input validation error: Invalid arguments for tool {tool}: `{field}` is a required property")
    })
}

fn required_str<'a>(tool: &str, args: &'a Value, field: &str) -> Result<&'a str, String> {
    required(tool, args, field)?.as_str().ok_or_else(|| {
        format!(
            "Input validation error: Invalid arguments for tool {tool}: `{field}` must be a string"
        )
    })
}

/// `stripe_context` and `livemode` must name this session's account (verbatim refusals).
fn check_context(ctx: &Ctx, tool: &str, args: &Value) -> Result<(), String> {
    let context = required_str(tool, args, "stripe_context")?;
    let livemode = required(tool, args, "livemode")?;
    let livemode = as_bool(livemode).ok_or_else(|| {
        format!("Input validation error: Invalid arguments for tool {tool}: `livemode` must be a boolean")
    })?;
    let account = ctx.account();
    let id = account["id"].as_str().unwrap_or_default();
    if context != id {
        return Err("No account found for the provided stripe_context and livemode. Use the list_available_accounts_or_orgs tool to see the accounts you can access.".to_owned());
    }
    let account_live = account["livemode"].as_bool().unwrap_or(false);
    if livemode != account_live {
        return Err(if account_live {
            format!("The provided account {id} is a live account. Retry with livemode set to true.")
        } else {
            format!(
                "The provided account {id} is a sandbox account. Retry with livemode set to false."
            )
        });
    }
    Ok(())
}

fn list_accounts(ctx: &Ctx) -> Value {
    let account = ctx.account();
    compact(&json!({
        "accounts": [{
            "stripe_context": account["id"],
            "livemode": account["livemode"],
            "name": ctx.account_name()
        }]
    }))
}

fn manage_accounts(ctx: &mut Ctx) -> Value {
    let session = ctx.raw_id("oauth_session", "oases_", 24);
    compact(&json!({ "reconsent_url": format!("{RECONSENT_URL}{session}") }))
}

fn not_available(op: &str) -> String {
    format!(
        "Operation '{op}' is not available. Use stripe_api_search to find available operations."
    )
}

fn api_search(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let intent = required_str("stripe_api_search", args, "intent")?;
    let resource = required_str("stripe_api_search", args, "resource")?;
    check_context(ctx, "stripe_api_search", args)?;
    let limit = args.get("limit").and_then(as_int).unwrap_or(5);
    if !(1..=20).contains(&limit) {
        return Err(format!(
            "Input validation error: Invalid arguments for tool stripe_api_search: `limit` must be between 1 and 20 (got {limit})"
        ));
    }
    Ok(compact(&catalog::search(intent, resource, limit as usize)))
}

fn api_details(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let op = required_str("stripe_api_details", args, "stripe_api_operation_id")?;
    check_context(ctx, "stripe_api_details", args)?;
    match catalog::find(op).filter(|o| o.mcp) {
        Some(entry) => Ok(compact(&entry.details())),
        None => Err(not_available(op)),
    }
}

fn render_error(err: &StripeError, op: &str) -> String {
    format!(
        "Stripe API error: {}\n\nUse stripe_api_details with stripe_api_operation_id: \"{op}\" to see all required and optional parameters.",
        err.message
    )
}

fn api_call(ctx: &mut Ctx, args: &Value, write: bool) -> Result<Value, String> {
    let tool = if write {
        "stripe_api_write"
    } else {
        "stripe_api_read"
    };
    let op = required_str(tool, args, "stripe_api_operation_id")?.to_owned();
    let parameters = required(tool, args, "parameters")?;
    if !parameters.is_object() {
        return Err(format!(
            "Input validation error: Invalid arguments for tool {tool}: `parameters` must be an object"
        ));
    }
    check_context(ctx, tool, args)?;
    let Some(entry) = catalog::find(&op).filter(|o| o.mcp) else {
        return Err(not_available(&op));
    };
    if (entry.method == "GET") == write {
        return Err(not_available(&op));
    }
    let route = api::route(&op).ok_or_else(|| not_available(&op))?;
    let mut params = Params::from_value(parameters);
    let result = api::args_from_params(route, &mut params)
        .and_then(|path_args| api::dispatch(ctx, route, &path_args, &params));
    match result {
        Ok(value) => Ok(compact(&value)),
        Err(err) => Err(render_error(&err, &op)),
    }
}

const ANALYTICS_INTENTS: &[&str] = &[
    "execute_query_run",
    "retrieve_query_run",
    "search_query_tables",
    "retrieve_query_table",
    "execute_query_template",
    "retrieve_query_template",
];

fn analytics(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let intent = required_str("stripe_analytics", args, "intent")?;
    if !ANALYTICS_INTENTS.contains(&intent) {
        return Err(format!(
            "Input validation error: Invalid arguments for tool stripe_analytics: `intent` must be one of {}",
            ANALYTICS_INTENTS.join(", ")
        ));
    }
    check_context(ctx, "stripe_analytics", args)?;
    Err(format!(
        "Stripe API error: {}",
        commerce::sigma_refusal().message
    ))
}

// --- documentation search ---------------------------------------------------------------

/// A small pinned corpus of Stripe documentation pages (summaries of docs.stripe.com).
const DOCS: &[(&str, &str, &str)] = &[
    (
        "Refund and cancel payments",
        "https://docs.stripe.com/refunds",
        "You can refund all or part of a payment by creating a Refund with the `charge` or `payment_intent` it applies to. Omit `amount` to refund the full remaining amount; partial refunds can be issued until the charge is fully refunded. Set `reason` to `duplicate`, `fraudulent` or `requested_by_customer`. Refunds use the original payment method and appear on the customer's statement within 5-10 business days. A charge that is disputed cannot be refunded.",
    ),
    (
        "Cancel subscriptions",
        "https://docs.stripe.com/billing/subscriptions/cancel",
        "Cancel a subscription immediately with DELETE /v1/subscriptions/:id, or at the end of the current period by updating it with `cancel_at_period_end=true`. A subscription scheduled to cancel keeps its `active` status until the period ends, and `cancel_at` shows when it will end. To undo a scheduled cancellation, set `cancel_at_period_end=false`. Use `cancellation_details` to record why the customer canceled.",
    ),
    (
        "Coupons and promotion codes",
        "https://docs.stripe.com/billing/subscriptions/coupons",
        "Coupons define a discount: `percent_off` or `amount_off` with a `currency`, and a `duration` of `once`, `repeating` (with `duration_in_months`) or `forever`. Apply a coupon to a subscription with the `discounts` parameter, for example `discounts[0][coupon]=SUMMER20`, or let customers redeem a customer-facing promotion code created from the coupon. Each application creates a Discount object on the subscription.",
    ),
    (
        "Subscription objects and billing periods",
        "https://docs.stripe.com/billing/subscriptions/overview",
        "A subscription has one or more items, each tied to a recurring Price and a quantity. Each item carries its `current_period_start` and `current_period_end`. Statuses include `trialing`, `active`, `past_due`, `unpaid`, `canceled`, `incomplete`, `incomplete_expired` and `paused`. List subscriptions with `status=all` to include canceled ones.",
    ),
    (
        "Monthly recurring revenue (MRR)",
        "https://docs.stripe.com/billing/subscriptions/analytics",
        "MRR normalizes recurring revenue to a monthly amount: for each active subscription item, multiply the price's unit amount by the quantity and convert the billing interval to a month (divide yearly prices by 12, multiply weekly prices by 52/12). Discounts reduce MRR; trials and canceled subscriptions do not contribute.",
    ),
    (
        "Search API query language",
        "https://docs.stripe.com/search",
        "Search customers, charges, invoices, payment intents, prices, products and subscriptions with a `query` such as `email:\"jenny@example.com\"`, `name~\"jen\"` (substring, at least 3 characters), `metadata[\"order_id\"]:\"6735\"` or `amount>1000`. Combine clauses with AND or OR (not both) and negate with a leading `-`. Search results may lag behind writes by up to a minute.",
    ),
    (
        "Pagination",
        "https://docs.stripe.com/api/pagination",
        "List endpoints return `{object: \"list\", data, has_more, url}` sorted by creation date, newest first. Pass `limit` (1-100, default 10) and `starting_after` with the last object's id to fetch the next page, or `ending_before` to page backwards.",
    ),
    (
        "Expanding responses",
        "https://docs.stripe.com/api/expanding_objects",
        "Many fields contain the id of a related object. Pass `expand[]=customer` to receive the full object instead, use dot notation for nested fields (`expand[]=latest_invoice.payment_intent`), and prefix with `data.` on list requests (`expand[]=data.customer`).",
    ),
    (
        "Invoices",
        "https://docs.stripe.com/invoicing/overview",
        "Invoices move from `draft` to `open` when finalized, then to `paid`, `void` or `uncollectible`. Add invoice items to a draft, finalize it to assign a number, and void an open invoice to cancel it. Invoices created by subscriptions link back through `parent.subscription_details.subscription`.",
    ),
    (
        "Disputes",
        "https://docs.stripe.com/disputes/responding",
        "When a customer disputes a charge, respond by updating the dispute with `evidence` before `evidence_details.due_by`. Submitting evidence moves the dispute to `under_review`. A disputed charge cannot be refunded.",
    ),
    (
        "Payment Links",
        "https://docs.stripe.com/payment-links",
        "Payment Links create a hosted payment page from `line_items` with a price and quantity. Deactivate a link by setting `active=false`.",
    ),
    (
        "Checkout Sessions",
        "https://docs.stripe.com/payments/checkout",
        "Create a Checkout Session with a `mode` (`payment`, `subscription` or `setup`), `line_items` and a `success_url`, then redirect the customer to the session's `url`. Sessions expire after 24 hours.",
    ),
    (
        "Balance and payouts",
        "https://docs.stripe.com/payouts",
        "Your balance has `available` and `pending` amounts per currency. Every movement of funds is recorded as a balance transaction (charge, refund, payout, adjustment). Payouts send available funds to your bank account.",
    ),
    (
        "Error codes",
        "https://docs.stripe.com/error-codes",
        "Errors return `{error: {type, code, message, param}}`. `resource_missing` means the id does not exist, `parameter_unknown` that a parameter is not accepted by the endpoint, `parameter_missing` that a required parameter was omitted, and `charge_already_refunded` that a refund was attempted on a fully refunded charge.",
    ),
];

fn search_docs(args: &Value) -> Result<Value, String> {
    let question = required_str("search_stripe_documentation", args, "question")?;
    if question.chars().count() > 1000 {
        return Err("Input validation error: Invalid arguments for tool search_stripe_documentation: `question` must be at most 1000 characters".to_owned());
    }
    let words: Vec<String> = question
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.trim_end_matches('s').to_owned())
        .collect();
    let mut scored: Vec<(usize, usize)> = DOCS
        .iter()
        .enumerate()
        .map(|(i, (title, _, body))| {
            let hay = format!("{title} {title} {body}").to_ascii_lowercase();
            (words.iter().filter(|w| hay.contains(w.as_str())).count(), i)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if scored.is_empty() {
        return Ok(mcp::text(
            "No relevant documentation found for this question.",
        ));
    }
    let text = scored
        .iter()
        .take(3)
        .map(|(_, i)| {
            let (title, url, body) = DOCS[*i];
            format!("## {title}\n{url}\n\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    Ok(mcp::text(text))
}

// --- implementation planner ---------------------------------------------------------------

fn planner(ctx: &mut Ctx, args: &Value) -> Result<Value, String> {
    check_context(ctx, "stripe_implementation_planner", args)?;
    let guide_id = args
        .get("guide_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let accept = args.get("accept").and_then(as_bool).unwrap_or(false);
    let selected: Vec<String> = args
        .get("selected_leaf_nodes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let id = match &guide_id {
        Some(id) => {
            if ctx.get("planner_guides", id).is_none() {
                return Err(format!(
                    "Guide {id} not found. Start a new planning session without guide_id."
                ));
            }
            id.clone()
        }
        None => ctx.raw_id("planner_guide", "guide_", 24),
    };
    let mut guide = ctx.get("planner_guides", &id).unwrap_or_else(
        || json!({"id": id, "messages": [], "selected_leaf_nodes": [], "accepted": false}),
    );
    if !message.is_empty() {
        guide["messages"]
            .as_array_mut()
            .expect("array")
            .push(json!(message));
    }
    if !selected.is_empty() {
        guide["selected_leaf_nodes"] = json!(selected);
    }
    if accept {
        guide["accepted"] = json!(true);
    }
    ctx.put("planner_guides", &id, guide.clone());
    let chosen: Vec<String> = guide["selected_leaf_nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let out = if guide["accepted"] == true {
        json!({
            "guide_id": id,
            "status": "accepted",
            "plan": {
                "selected": chosen,
                "steps": [
                    "Create Products and Prices for what you sell (stripe_api_write PostProducts, PostPrices).",
                    "Collect payment with a Checkout Session (PostCheckoutSessions) or a Payment Link (PostPaymentLinks).",
                    "For recurring revenue, create Subscriptions on those Prices (PostSubscriptions) and let Stripe Billing generate invoices.",
                    "Listen for webhook events (PostWebhookEndpoints) such as checkout.session.completed and invoice.paid to fulfil orders.",
                    "Test end to end in the sandbox before switching to live mode."
                ]
            }
        })
    } else {
        json!({
            "guide_id": id,
            "status": "needs_selection",
            "question": "Which integration best describes what you want to build?",
            "decision_tree": {
                "id": "root",
                "children": [
                    {"id": "one_time_payments", "label": "Accept one-time payments", "children": [
                        {"id": "checkout_hosted", "label": "Stripe-hosted Checkout page"},
                        {"id": "payment_links", "label": "No-code Payment Links"},
                        {"id": "payment_element", "label": "Embedded Payment Element"}
                    ]},
                    {"id": "subscriptions", "label": "Recurring billing", "children": [
                        {"id": "billing_checkout", "label": "Subscriptions with Checkout"},
                        {"id": "billing_invoices", "label": "Invoiced subscriptions"}
                    ]},
                    {"id": "marketplace", "label": "Platform or marketplace", "children": [
                        {"id": "connect_express", "label": "Connect with Express accounts"}
                    ]}
                ]
            },
            "selected_leaf_nodes": chosen,
            "next": "Call again with guide_id and selected_leaf_nodes, then accept=true to receive the plan."
        })
    };
    Ok(compact(&out))
}

// --- feedback -----------------------------------------------------------------------------

fn feedback(ctx: &mut Ctx, args: &Value) -> Result<Value, String> {
    let tool = "send_stripe_mcp_feedback";
    let sentiment = required_str(tool, args, "sentiment")?;
    let quote = required_str(tool, args, "quote")?;
    let context = required_str(tool, args, "context")?;
    let source = required_str(tool, args, "source")?;
    if !["positive", "negative", "neutral"].contains(&sentiment) {
        return Err(format!(
            "Input validation error: Invalid arguments for tool {tool}: `sentiment` must be one of positive, negative, neutral"
        ));
    }
    if !["user", "agent"].contains(&source) {
        return Err(format!(
            "Input validation error: Invalid arguments for tool {tool}: `source` must be one of user, agent"
        ));
    }
    if quote.is_empty() || quote.chars().count() > 1000 {
        return Err(format!(
            "Input validation error: Invalid arguments for tool {tool}: `quote` must be between 1 and 1000 characters"
        ));
    }
    let id = ctx.raw_id("mcp_feedback", "fb_", 24);
    ctx.put(
        "mcp_feedback",
        &id,
        json!({
            "id": id,
            "sentiment": sentiment,
            "quote": quote,
            "context": context,
            "source": source,
            "tool_name": args.get("tool_name").cloned().unwrap_or(Value::Null),
            "created": ctx.now()
        }),
    );
    Ok(mcp::text(
        "Thank you for your feedback! It has been shared with the Stripe MCP team.",
    ))
}
