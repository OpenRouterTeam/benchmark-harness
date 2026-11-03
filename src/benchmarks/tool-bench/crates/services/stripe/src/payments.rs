//! Customers, payment methods, payment intents, charges, refunds, disputes, balance,
//! balance transactions, payouts and events.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::{
    api::{Args, deleted},
    engine::{Ctx, eq_param, range_match, sort_newest_first},
    error::{Res, StripeError, money},
    params::{Params, apply_metadata, scalar_string},
};

// --- shared helpers -----------------------------------------------------------------

pub fn retrieve(ctx: &mut Ctx, object: &str, id: &str, params: &Params) -> Res<Value> {
    let doc = ctx.require(object, id, "id")?;
    ctx.present(object, &doc, &params.expand())
}

/// Copies a string parameter onto `doc`; `""` unsets it (null).
pub fn set_str(doc: &mut Value, params: &Params, key: &str) {
    if let Some(v) = params.raw(key) {
        doc[key] = match scalar_string(v) {
            Some(s) if s.is_empty() => Value::Null,
            Some(s) => json!(s),
            None => v.clone(),
        };
    }
}

/// Copies a structured parameter onto `doc`; `""` unsets it.
pub fn set_any(doc: &mut Value, params: &Params, key: &str) {
    if let Some(v) = params.raw(key) {
        doc[key] = if v.as_str() == Some("") {
            Value::Null
        } else {
            v.clone()
        };
    }
}

pub fn set_int(doc: &mut Value, params: &Params, key: &str) -> Res<()> {
    if let Some(v) = params.int(key)? {
        doc[key] = json!(v);
    }
    Ok(())
}

pub fn set_bool(doc: &mut Value, params: &Params, key: &str) -> Res<()> {
    if let Some(v) = params.bool(key)? {
        doc[key] = json!(v);
    }
    Ok(())
}

pub fn set_metadata(doc: &mut Value, params: &Params) {
    if params.has("metadata") {
        let mut current = doc.get("metadata").cloned().unwrap_or_else(|| json!({}));
        apply_metadata(&mut current, params.raw("metadata"));
        doc["metadata"] = current;
    }
}

/// Top-level fields of `before` that differ in `after` (an event's `previous_attributes`).
pub fn previous(before: &Value, after: &Value) -> Value {
    let mut out = Map::new();
    if let (Some(b), Some(a)) = (before.as_object(), after.as_object()) {
        for (k, v) in a {
            if b.get(k) != Some(v) {
                out.insert(k.clone(), b.get(k).cloned().unwrap_or(Value::Null));
            }
        }
    }
    Value::Object(out)
}

pub fn check_enum(params: &Params, key: &str, allowed: &[&str]) -> Res<()> {
    if let Some(v) = params.str(key)
        && !v.is_empty()
        && !allowed.contains(&v.as_str())
    {
        return Err(StripeError::invalid_enum(key, allowed));
    }
    Ok(())
}

/// Requires a referenced object named by a body/query parameter (400, not 404).
pub fn require_ref(ctx: &Ctx, object: &str, id: &str, param: &str) -> Res<Value> {
    ctx.require(object, id, param).map_err(|e| e.status(400))
}

// --- customers ----------------------------------------------------------------------

pub fn list_customers(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("customer", "/v1/customers", params, &|d| {
        eq_param(d, "email", params) && eq_param(d, "test_clock", params)
    })
}

pub fn search_customers(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "customer",
        "customers",
        params,
        &["created", "email", "metadata", "name", "phone"],
    )
}

pub fn get_customer(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "customer", args.id(), params)
}

const CUSTOMER_STRINGS: &[&str] = &[
    "description",
    "email",
    "name",
    "phone",
    "business_name",
    "individual_name",
];

fn apply_customer_fields(ctx: &Ctx, doc: &mut Value, params: &Params) -> Res<()> {
    check_enum(params, "tax_exempt", &["none", "exempt", "reverse"])?;
    for key in CUSTOMER_STRINGS {
        set_str(doc, params, key);
    }
    set_str(doc, params, "tax_exempt");
    for key in ["address", "shipping", "preferred_locales"] {
        set_any(doc, params, key);
    }
    set_int(doc, params, "balance")?;
    set_int(doc, params, "next_invoice_sequence")?;
    if let Some(prefix) = params.str("invoice_prefix") {
        let valid = (1..=12).contains(&prefix.len())
            && prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if !valid {
            return Err(StripeError::invalid(
                "Invoice number prefix must be 1–12 uppercase letters or numbers.",
            )
            .param("invoice_prefix"));
        }
        if let Some((holder, _)) = ctx.docs("customers").into_iter().find(|(k, c)| {
            c["invoice_prefix"] == prefix.as_str() && Some(k.as_str()) != doc["id"].as_str()
        }) {
            return Err(StripeError::invalid(format!(
                "This invoice number prefix is taken by customer: {holder}. Please enter a different prefix."
            ))
            .param("invoice_prefix"));
        }
        doc["invoice_prefix"] = json!(prefix);
    }
    if let Some(settings) = params.obj("invoice_settings") {
        let mut current = doc
            .get("invoice_settings")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| {
                json!({"custom_fields": null, "default_payment_method": null, "footer": null, "rendering_options": null})
            });
        for (k, v) in settings {
            if k == "default_payment_method"
                && let Some(pm) = v.as_str().filter(|s| !s.is_empty())
            {
                require_ref(
                    ctx,
                    "payment_method",
                    pm,
                    "invoice_settings[default_payment_method]",
                )?;
            }
            current[k.as_str()] = if v.as_str() == Some("") {
                Value::Null
            } else {
                v.clone()
            };
        }
        doc["invoice_settings"] = current;
    }
    set_metadata(doc, params);
    Ok(())
}

pub fn create_customer(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let mut doc = json!({"id": "", "object": "customer", "metadata": {}});
    apply_customer_fields(ctx, &mut doc, params)?;
    let pm = params.str("payment_method").filter(|s| !s.is_empty());
    if let Some(pm) = &pm {
        require_ref(ctx, "payment_method", pm, "payment_method")?;
    }
    let id = ctx.id("customer");
    doc["id"] = json!(id);
    doc["created"] = json!(ctx.now());
    if doc.get("invoice_prefix").is_none_or(Value::is_null) {
        let prefix = ctx
            .raw_id("invoice_prefix", "", 8)
            .to_ascii_uppercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "X");
        doc["invoice_prefix"] = json!(prefix);
    }
    if let Some(pm) = pm
        && let Some(mut pm_doc) = ctx.lookup("payment_method", &pm)
    {
        pm_doc["customer"] = json!(id);
        ctx.put("payment_methods", &pm, pm_doc);
    }
    ctx.put("customers", &id, doc.clone());
    ctx.emit("customer.created", "customer", &doc, None);
    ctx.present("customer", &doc, &params.expand())
}

pub fn update_customer(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("customer", &id, "id")?;
    if before["deleted"] == true {
        return Err(StripeError::missing("customer", &id, "id"));
    }
    let mut doc = before.clone();
    apply_customer_fields(ctx, &mut doc, params)?;
    ctx.put("customers", &id, doc.clone());
    ctx.emit(
        "customer.updated",
        "customer",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("customer", &doc, &params.expand())
}

pub fn delete_customer(ctx: &mut Ctx, args: &Args, _: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let doc = ctx.require("customer", &id, "id")?;
    if doc["deleted"] == true {
        return Err(StripeError::missing("customer", &id, "id"));
    }
    let now = ctx.now();
    for (key, mut sub) in ctx.docs("subscriptions") {
        if sub["customer"] == id.as_str() && sub["status"] != "canceled" {
            sub["status"] = json!("canceled");
            sub["canceled_at"] = json!(now);
            sub["ended_at"] = json!(now);
            ctx.put("subscriptions", &key, sub.clone());
            ctx.emit("customer.subscription.deleted", "subscription", &sub, None);
        }
    }
    let stub = deleted(&id, "customer");
    ctx.put("customers", &id, stub.clone());
    ctx.emit("customer.deleted", "customer", &doc, None);
    Ok(stub)
}

fn live_customer(ctx: &Ctx, id: &str) -> Res<Value> {
    let doc = ctx.require("customer", id, "id")?;
    if doc["deleted"] == true {
        return Err(StripeError::missing("customer", id, "id"));
    }
    Ok(doc)
}

pub fn list_customer_balance_transactions(
    ctx: &mut Ctx,
    args: &Args,
    params: &Params,
) -> Res<Value> {
    let id = args.id().to_owned();
    live_customer(ctx, &id)?;
    ctx.list(
        "customer_balance_transaction",
        &format!("/v1/customers/{id}/balance_transactions"),
        params,
        &|d| d["customer"] == id.as_str() && eq_param(d, "invoice", params),
    )
}

pub fn get_cash_balance(ctx: &mut Ctx, args: &Args, _: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    live_customer(ctx, &id)?;
    Ok(json!({
        "object": "cash_balance",
        "available": null,
        "customer": id,
        "customer_account": null,
        "livemode": ctx.livemode(),
        "settings": {"reconciliation_mode": "automatic", "using_merchant_default": true}
    }))
}

pub fn list_cash_balance_transactions(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    live_customer(ctx, &id)?;
    ctx.list(
        "customer_cash_balance_transaction",
        &format!("/v1/customers/{id}/cash_balance_transactions"),
        params,
        &|d| d["customer"] == id.as_str(),
    )
}

pub fn list_customer_payment_methods(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    live_customer(ctx, &id)?;
    ctx.list(
        "payment_method",
        &format!("/v1/customers/{id}/payment_methods"),
        params,
        &|d| {
            d["customer"] == id.as_str()
                && params
                    .str("type")
                    .is_none_or(|t| d["type"].as_str().unwrap_or("card") == t)
        },
    )
}

pub fn get_customer_payment_method(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let customer = args.get("customer").to_owned();
    let pm = args.get("payment_method").to_owned();
    live_customer(ctx, &customer)?;
    let doc = ctx.require("payment_method", &pm, "id")?;
    if doc["customer"] != customer.as_str() {
        return Err(StripeError::missing("PaymentMethod", &pm, "id"));
    }
    ctx.present("payment_method", &doc, &params.expand())
}

// --- payment methods, intents, charges ----------------------------------------------

pub fn list_payment_methods(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("payment_method", "/v1/payment_methods", params, &|d| {
        eq_param(d, "customer", params)
            && params
                .str("type")
                .is_none_or(|t| d["type"].as_str().unwrap_or("card") == t)
    })
}

pub fn get_payment_method(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "payment_method", args.id(), params)
}

pub fn list_payment_intents(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("payment_intent", "/v1/payment_intents", params, &|d| {
        eq_param(d, "customer", params)
    })
}

pub fn search_payment_intents(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "payment_intent",
        "payment_intents",
        params,
        &[
            "amount", "created", "currency", "customer", "metadata", "status",
        ],
    )
}

pub fn get_payment_intent(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "payment_intent", args.id(), params)
}

pub fn list_charges(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("charge", "/v1/charges", params, &|d| {
        eq_param(d, "customer", params)
            && eq_param(d, "payment_intent", params)
            && eq_param(d, "transfer_group", params)
    })
}

pub fn search_charges(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.search(
        "charge",
        "charges",
        params,
        &[
            "amount",
            "billing_details.address.postal_code",
            "created",
            "currency",
            "customer",
            "disputed",
            "metadata",
            "payment_intent",
            "payment_method_details.card.brand",
            "payment_method_details.card.exp_month",
            "payment_method_details.card.exp_year",
            "payment_method_details.card.fingerprint",
            "payment_method_details.card.last4",
            "refunded",
            "status",
        ],
    )
}

pub fn get_charge(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "charge", args.id(), params)
}

pub fn list_charge_refunds(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    ctx.require("charge", &id, "id")?;
    ctx.list(
        "refund",
        &format!("/v1/charges/{id}/refunds"),
        params,
        &|d| d["charge"] == id.as_str(),
    )
}

// --- refunds ------------------------------------------------------------------------

pub fn list_refunds(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("refund", "/v1/refunds", params, &|d| {
        eq_param(d, "charge", params) && eq_param(d, "payment_intent", params)
    })
}

pub fn get_refund(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "refund", args.id(), params)
}

pub fn create_refund(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let charge_id = match (
        params.str("charge").filter(|s| !s.is_empty()),
        params.str("payment_intent").filter(|s| !s.is_empty()),
    ) {
        (Some(charge), _) => {
            require_ref(ctx, "charge", &charge, "charge")?;
            charge
        }
        (None, Some(pi)) => {
            let intent = require_ref(ctx, "payment_intent", &pi, "payment_intent")?;
            let latest = intent["latest_charge"].as_str().map(str::to_owned);
            let charge = latest
                .and_then(|c| ctx.lookup("charge", &c))
                .filter(|c| ctx.render("charge", c)["status"] == "succeeded");
            match charge {
                Some(c) => c["id"].as_str().unwrap_or_default().to_owned(),
                None => {
                    return Err(StripeError::invalid(format!(
                        "This PaymentIntent ({pi}) does not have a successful charge to refund."
                    )));
                }
            }
        }
        (None, None) => {
            return Err(StripeError::invalid(
                "One of the following params should be provided for this request: payment_intent or charge.",
            ));
        }
    };
    check_enum(
        params,
        "reason",
        &["duplicate", "fraudulent", "requested_by_customer"],
    )
    .map_err(|_| {
        StripeError::invalid(
            "Invalid reason: must be one of duplicate, fraudulent, or requested_by_customer",
        )
        .param("reason")
    })?;
    let stored = ctx.lookup("charge", &charge_id).expect("checked above");
    let charge = ctx.render("charge", &stored);
    if charge["disputed"] == true {
        return Err(StripeError::invalid(format!(
            "Charge {charge_id} has been charged back; cannot issue a refund."
        ))
        .code("charge_disputed"));
    }
    let amount = charge["amount"].as_i64().unwrap_or(0);
    let refunded = charge["amount_refunded"].as_i64().unwrap_or(0);
    let currency = charge["currency"].as_str().unwrap_or("usd").to_owned();
    if charge["refunded"] == true || refunded >= amount {
        return Err(
            StripeError::invalid(format!("Charge {charge_id} has already been refunded."))
                .code("charge_already_refunded"),
        );
    }
    let remaining = amount - refunded;
    let requested = params.int("amount")?.unwrap_or(remaining);
    if requested < 1 {
        return Err(
            StripeError::invalid("This value must be greater than or equal to 1.")
                .code("parameter_invalid_integer")
                .param("amount"),
        );
    }
    if requested > remaining {
        return Err(StripeError::invalid(format!(
            "Refund amount ({}) is greater than unrefunded amount on charge ({})",
            money(&currency, requested),
            money(&currency, remaining)
        ))
        .param("amount"));
    }

    let now = ctx.now();
    let refund_id = ctx.id("refund");
    let txn_id = ctx.id("balance_transaction");
    let mut refund = json!({
        "id": refund_id,
        "object": "refund",
        "amount": requested,
        "balance_transaction": txn_id,
        "charge": charge_id,
        "created": now,
        "currency": currency,
        "metadata": {},
        "payment_intent": charge["payment_intent"],
        "reason": params.str("reason").filter(|s| !s.is_empty()),
        "status": "succeeded"
    });
    set_metadata(&mut refund, params);
    ctx.put("refunds", &refund_id, refund.clone());
    let description = charge["description"]
        .as_str()
        .map(|d| format!("REFUND FOR CHARGE ({d})"))
        .unwrap_or_else(|| "REFUND FOR CHARGE".to_owned());
    let txn = json!({
        "id": txn_id,
        "object": "balance_transaction",
        "amount": -requested,
        "available_on": now,
        "created": now,
        "currency": currency,
        "description": description,
        "fee": 0,
        "fee_details": [],
        "net": -requested,
        "reporting_category": "refund",
        "source": refund_id,
        "status": "available",
        "type": "refund"
    });
    ctx.put("balance_transactions", &txn_id, txn);
    let mut updated = stored.clone();
    updated["amount_refunded"] = json!(refunded + requested);
    updated["refunded"] = json!(refunded + requested >= amount);
    ctx.put("charges", &charge_id, updated.clone());
    ctx.emit("refund.created", "refund", &refund, None);
    ctx.emit(
        "charge.refunded",
        "charge",
        &updated,
        Some(json!({"amount_refunded": refunded, "refunded": false})),
    );
    ctx.present("refund", &refund, &params.expand())
}

// --- disputes -----------------------------------------------------------------------

pub fn list_disputes(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("dispute", "/v1/disputes", params, &|d| {
        eq_param(d, "charge", params) && eq_param(d, "payment_intent", params)
    })
}

pub fn get_dispute(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "dispute", args.id(), params)
}

pub fn update_dispute(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("dispute", &id, "id")?;
    let current = ctx.render("dispute", &before);
    let status = current["status"]
        .as_str()
        .unwrap_or("needs_response")
        .to_owned();
    if matches!(
        status.as_str(),
        "won" | "lost" | "warning_closed" | "prevented"
    ) {
        return Err(StripeError::invalid(format!(
            "This dispute is already closed: {id}"
        )));
    }
    if matches!(status.as_str(), "under_review" | "warning_under_review") && params.has("evidence")
    {
        return Err(StripeError::invalid(
            "This dispute has already been submitted and can no longer be updated.",
        ));
    }
    let mut doc = before.clone();
    if let Some(evidence) = params.obj("evidence") {
        let mut merged = current["evidence"].clone();
        for (k, v) in evidence {
            merged[k.as_str()] = if v.as_str() == Some("") {
                Value::Null
            } else {
                v
            };
        }
        doc["evidence"] = merged;
        let mut details = current["evidence_details"].clone();
        details["has_evidence"] = json!(true);
        doc["evidence_details"] = details;
    }
    set_metadata(&mut doc, params);
    if params.bool("submit")?.unwrap_or(true) && params.has("evidence") {
        doc["status"] = json!(if status.starts_with("warning") {
            "warning_under_review"
        } else {
            "under_review"
        });
        let mut details = doc["evidence_details"].clone();
        if details.is_null() {
            details = current["evidence_details"].clone();
        }
        details["submission_count"] = json!(details["submission_count"].as_i64().unwrap_or(0) + 1);
        doc["evidence_details"] = details;
    }
    ctx.put("disputes", &id, doc.clone());
    ctx.emit(
        "charge.dispute.updated",
        "dispute",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("dispute", &doc, &params.expand())
}

// --- balance, payouts, events -------------------------------------------------------

pub fn get_balance(ctx: &mut Ctx, _: &Args, _: &Params) -> Res<Value> {
    let txns: Vec<Value> = ctx
        .docs("balance_transactions")
        .into_iter()
        .map(|(_, v)| ctx.render("balance_transaction", &v))
        .collect();
    let base = ctx.docs("balance").into_iter().next().map(|(_, v)| v);
    let livemode = ctx.livemode();
    if txns.is_empty()
        && let Some(mut base) = base
    {
        base["object"] = json!("balance");
        base["livemode"] = json!(livemode);
        return Ok(crate::engine::sorted(base));
    }
    let mut available: BTreeMap<String, i64> = BTreeMap::new();
    let mut pending: BTreeMap<String, i64> = BTreeMap::new();
    let now = ctx.now();
    for t in &txns {
        let currency = t["currency"].as_str().unwrap_or("usd").to_owned();
        let net = t["net"].as_i64().unwrap_or(0);
        let is_available =
            t["status"] == "available" || t["available_on"].as_i64().is_some_and(|a| a <= now);
        let bucket = if is_available {
            &mut available
        } else {
            &mut pending
        };
        *bucket.entry(currency.clone()).or_default() += net;
        available.entry(currency.clone()).or_default();
        pending.entry(currency).or_default();
    }
    if available.is_empty() {
        available.insert("usd".into(), 0);
        pending.insert("usd".into(), 0);
    }
    let amounts = |m: &BTreeMap<String, i64>| -> Vec<Value> {
        m.iter()
            .map(|(c, a)| json!({"amount": a, "currency": c, "source_types": {"card": a}}))
            .collect()
    };
    Ok(json!({
        "object": "balance",
        "available": amounts(&available),
        "livemode": livemode,
        "pending": amounts(&pending),
        "refund_and_dispute_prefunding": {
            "available": available.keys().map(|c| json!({"amount": 0, "currency": c})).collect::<Vec<_>>(),
            "pending": pending.keys().map(|c| json!({"amount": 0, "currency": c})).collect::<Vec<_>>()
        }
    }))
}

pub fn list_balance_transactions(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let payout_txns: Option<Vec<String>> = params.str("payout").map(|po| {
        ctx.docs("balance_transactions")
            .into_iter()
            .filter(|(_, t)| t["payout"] == po.as_str())
            .map(|(k, _)| k)
            .collect()
    });
    ctx.list(
        "balance_transaction",
        "/v1/balance_transactions",
        params,
        &|d| {
            eq_param(d, "currency", params)
                && eq_param(d, "source", params)
                && params
                    .str("type")
                    .is_none_or(|t| d["type"].as_str().unwrap_or("charge") == t)
                && payout_txns.as_ref().is_none_or(|ids| {
                    d["id"]
                        .as_str()
                        .is_some_and(|id| ids.iter().any(|i| i == id))
                })
        },
    )
}

pub fn get_balance_transaction(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "balance_transaction", args.id(), params)
}

pub fn list_payouts(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("payout", "/v1/payouts", params, &|d| {
        let r = ctx.render("payout", d);
        eq_param(&r, "status", params)
            && eq_param(&r, "destination", params)
            && params
                .raw("arrival_date")
                .is_none_or(|f| range_match(&r["arrival_date"], f))
    })
}

pub fn get_payout(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "payout", args.id(), params)
}

fn type_matches(pattern: &str, event_type: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => event_type.starts_with(prefix),
        None => pattern == event_type,
    }
}

pub fn list_events(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let types: Vec<String> = params
        .list("types")
        .iter()
        .filter_map(scalar_string)
        .collect();
    let single = params.str("type");
    ctx.list("event", "/v1/events", params, &|d| {
        let t = d["type"].as_str().unwrap_or_default();
        single.as_deref().is_none_or(|p| type_matches(p, t))
            && (types.is_empty() || types.iter().any(|p| type_matches(p, t)))
    })
}

pub fn get_event(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "event", args.id(), params)
}

/// Sorted copies of a collection's documents matching `filter` (newest first).
pub fn matching(ctx: &Ctx, collection: &str, filter: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut docs: Vec<Value> = ctx
        .docs(collection)
        .into_iter()
        .map(|(_, v)| v)
        .filter(|d| filter(d))
        .collect();
    sort_newest_first(&mut docs);
    docs
}
