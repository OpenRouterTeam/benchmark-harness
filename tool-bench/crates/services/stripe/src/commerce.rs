//! Payment Links, Checkout Sessions, the customer portal, settings-style resources
//! (webhook endpoints, payment method domains and configurations, test clocks, tax codes),
//! Connect reads, and the refusals for products this account has not activated.

use serde_json::{Value, json};

use crate::{
    api::Args,
    engine::{Ctx, eq_param, short_hash},
    error::{Res, StripeError},
    params::{Params, as_int, scalar_string},
    payments::{
        check_enum, previous, require_ref, retrieve, set_any, set_bool, set_metadata, set_str,
    },
};

// --- Connect and other reads ----------------------------------------------------------

pub fn list_accounts(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("account", "/v1/accounts", params, &|_| true)
}

pub fn get_account(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let own = ctx.account();
    if own["id"] == id.as_str() {
        return Ok(crate::engine::sorted(own));
    }
    match ctx.lookup("account", &id) {
        Some(doc) => ctx.present("account", &doc, &params.expand()),
        None => Err(StripeError::invalid(format!(
            "The provided key does not have access to account '{id}' (or that account does not exist). Application access may have been revoked."
        ))
        .code("account_invalid")
        .status(403)),
    }
}

pub fn list_application_fees(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("application_fee", "/v1/application_fees", params, &|d| {
        eq_param(d, "charge", params)
    })
}

pub fn list_transfers(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("transfer", "/v1/transfers", params, &|d| {
        eq_param(d, "destination", params) && eq_param(d, "transfer_group", params)
    })
}

pub fn get_transfer(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "transfer", args.id(), params)
}

pub fn list_setup_intents(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("setup_intent", "/v1/setup_intents", params, &|d| {
        eq_param(d, "customer", params) && eq_param(d, "payment_method", params)
    })
}

pub fn get_setup_intent(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "setup_intent", args.id(), params)
}

pub fn list_subscription_schedules(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list(
        "subscription_schedule",
        "/v1/subscription_schedules",
        params,
        &|d| eq_param(d, "customer", params),
    )
}

pub fn get_subscription_schedule(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "subscription_schedule", args.id(), params)
}

pub fn list_credit_notes(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("credit_note", "/v1/credit_notes", params, &|d| {
        eq_param(d, "customer", params) && eq_param(d, "invoice", params)
    })
}

// --- line items (Payment Links and Checkout) -------------------------------------------

/// `item` objects for `line_items` parameters: `[{price, quantity}]`.
fn line_items(ctx: &mut Ctx, params: &Params, required: bool) -> Res<(Vec<Value>, String)> {
    let entries = params.list("line_items");
    if entries.is_empty() && required {
        return Err(StripeError::missing_param("line_items"));
    }
    let mut items = Vec::new();
    let mut currency: Option<String> = None;
    for (i, entry) in entries.iter().enumerate() {
        let price_id = entry
            .get("price")
            .and_then(scalar_string)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| StripeError::missing_param(&format!("line_items[{i}][price]")))?;
        let price = require_ref(ctx, "price", &price_id, &format!("line_items[{i}][price]"))?;
        let price = ctx.render("price", &price);
        let quantity = entry.get("quantity").and_then(as_int).unwrap_or(1);
        let c = price["currency"].as_str().unwrap_or("usd").to_owned();
        if currency.as_ref().is_some_and(|existing| *existing != c) {
            return Err(
                StripeError::invalid("All line items must use the same currency.")
                    .param(&format!("line_items[{i}][price]")),
            );
        }
        currency = Some(c.clone());
        let amount = price["unit_amount"].as_i64().unwrap_or(0) * quantity;
        let description = ctx
            .lookup("product", price["product"].as_str().unwrap_or_default())
            .and_then(|p| p["name"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let id = ctx.raw_id("checkout_line_item", "li_", 24);
        items.push(json!({
            "id": id,
            "object": "item",
            "adjustable_quantity": null,
            "amount_discount": 0,
            "amount_subtotal": amount,
            "amount_tax": 0,
            "amount_total": amount,
            "currency": c,
            "description": description,
            "metadata": {},
            "price": price,
            "quantity": quantity
        }));
    }
    Ok((items, currency.unwrap_or_else(|| "usd".into())))
}

fn stored_line_items(ctx: &Ctx, object: &str, id: &str) -> Res<Vec<Value>> {
    let doc = ctx.require(object, id, "id")?;
    Ok(doc["_line_items"].as_array().cloned().unwrap_or_default())
}

// --- Payment Links ----------------------------------------------------------------------

pub fn list_payment_links(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let active = params.bool("active")?;
    ctx.list("payment_link", "/v1/payment_links", params, &|d| {
        active.is_none_or(|a| d["active"].as_bool().unwrap_or(true) == a)
    })
}

pub fn get_payment_link(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "payment_link", args.id(), params)
}

pub fn payment_link_line_items(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let items = stored_line_items(ctx, "payment_link", &id)?;
    ctx.page(
        "item",
        &format!("/v1/payment_links/{id}/line_items"),
        items,
        params,
    )
}

const LINK_FIELDS: &[&str] = &[
    "after_completion",
    "automatic_tax",
    "consent_collection",
    "custom_fields",
    "custom_text",
    "invoice_creation",
    "payment_intent_data",
    "payment_method_types",
    "phone_number_collection",
    "restrictions",
    "shipping_address_collection",
    "shipping_options",
    "subscription_data",
    "tax_id_collection",
];

pub fn create_payment_link(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let (items, currency) = line_items(ctx, params, true)?;
    check_enum(params, "billing_address_collection", &["auto", "required"])?;
    check_enum(
        params,
        "submit_type",
        &["auto", "book", "donate", "pay", "subscribe"],
    )?;
    let id = ctx.id("payment_link");
    let mut doc = json!({
        "id": id,
        "object": "payment_link",
        "active": true,
        "created": ctx.now(),
        "currency": currency,
        "metadata": {},
        "url": format!("https://buy.stripe.com/test_{}", short_hash(&id)),
        "_line_items": items
    });
    for key in LINK_FIELDS {
        set_any(&mut doc, params, key);
    }
    for key in [
        "billing_address_collection",
        "customer_creation",
        "inactive_message",
        "payment_method_collection",
        "submit_type",
    ] {
        set_str(&mut doc, params, key);
    }
    set_bool(&mut doc, params, "allow_promotion_codes")?;
    set_metadata(&mut doc, params);
    ctx.put("payment_links", &id, doc.clone());
    ctx.emit("payment_link.created", "payment_link", &doc, None);
    ctx.present("payment_link", &doc, &params.expand())
}

pub fn update_payment_link(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("payment_link", &id, "id")?;
    let mut doc = before.clone();
    set_bool(&mut doc, params, "active")?;
    set_bool(&mut doc, params, "allow_promotion_codes")?;
    for key in LINK_FIELDS {
        set_any(&mut doc, params, key);
    }
    for key in [
        "billing_address_collection",
        "customer_creation",
        "inactive_message",
        "payment_method_collection",
        "submit_type",
    ] {
        set_str(&mut doc, params, key);
    }
    let updates = params.list("line_items");
    if !updates.is_empty() {
        let mut items = doc["_line_items"].as_array().cloned().unwrap_or_default();
        for (i, u) in updates.iter().enumerate() {
            let item_id = u.get("id").and_then(scalar_string).unwrap_or_default();
            let Some(item) = items.iter_mut().find(|it| it["id"] == item_id.as_str()) else {
                return Err(
                    StripeError::invalid(format!("No such line item: '{item_id}'"))
                        .param(&format!("line_items[{i}][id]")),
                );
            };
            if let Some(q) = u.get("quantity").and_then(as_int) {
                let unit = item["price"]["unit_amount"].as_i64().unwrap_or(0);
                item["quantity"] = json!(q);
                item["amount_subtotal"] = json!(unit * q);
                item["amount_total"] = json!(unit * q);
            }
        }
        doc["_line_items"] = json!(items);
    }
    set_metadata(&mut doc, params);
    ctx.put("payment_links", &id, doc.clone());
    ctx.emit(
        "payment_link.updated",
        "payment_link",
        &doc,
        Some(previous(
            &ctx.render("payment_link", &before),
            &ctx.render("payment_link", &doc),
        )),
    );
    ctx.present("payment_link", &doc, &params.expand())
}

// --- Checkout Sessions ------------------------------------------------------------------

pub fn list_checkout_sessions(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    check_enum(params, "status", &["complete", "expired", "open"])?;
    ctx.list("checkout.session", "/v1/checkout/sessions", params, &|d| {
        let r = ctx.render("checkout.session", d);
        eq_param(&r, "customer", params)
            && eq_param(&r, "payment_intent", params)
            && eq_param(&r, "payment_link", params)
            && eq_param(&r, "status", params)
            && eq_param(&r, "subscription", params)
    })
}

pub fn get_checkout_session(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "checkout.session", args.id(), params)
}

pub fn checkout_session_line_items(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let items = stored_line_items(ctx, "checkout.session", &id)?;
    ctx.page(
        "item",
        &format!("/v1/checkout/sessions/{id}/line_items"),
        items,
        params,
    )
}

pub fn create_checkout_session(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let mode = params.req_str("mode")?;
    if !["payment", "setup", "subscription"].contains(&mode.as_str()) {
        return Err(StripeError::invalid_enum(
            "mode",
            &["payment", "setup", "subscription"],
        ));
    }
    let (items, currency) = line_items(ctx, params, mode != "setup")?;
    if let Some(customer) = params.str("customer").filter(|s| !s.is_empty()) {
        require_ref(ctx, "customer", &customer, "customer")?;
    }
    let total: i64 = items
        .iter()
        .filter_map(|i| i["amount_total"].as_i64())
        .sum();
    let now = ctx.now();
    let id = ctx.id("checkout.session");
    let mut doc = json!({
        "id": id,
        "object": "checkout.session",
        "allow_promotion_codes": params.bool("allow_promotion_codes")?,
        "amount_subtotal": if mode == "setup" { Value::Null } else { json!(total) },
        "amount_total": if mode == "setup" { Value::Null } else { json!(total) },
        "billing_address_collection": params.str("billing_address_collection"),
        "cancel_url": params.str("cancel_url"),
        "client_reference_id": params.str("client_reference_id"),
        "created": now,
        "currency": if mode == "setup" { params.str("currency").map(Value::from).unwrap_or(Value::Null) } else { json!(currency) },
        "customer": params.str("customer").filter(|s| !s.is_empty()),
        "customer_creation": if params.has("customer") { Value::Null } else { json!("if_required") },
        "customer_email": params.str("customer_email"),
        "expires_at": params.int("expires_at")?.unwrap_or(now + 86_400),
        "locale": params.str("locale"),
        "metadata": {},
        "mode": mode,
        "payment_method_types": ["card"],
        "payment_status": if mode == "setup" { "no_payment_required" } else { "unpaid" },
        "status": "open",
        "submit_type": params.str("submit_type"),
        "success_url": params.str("success_url"),
        "total_details": {"amount_discount": 0, "amount_shipping": 0, "amount_tax": 0},
        "ui_mode": params.str("ui_mode").unwrap_or_else(|| "hosted".into()),
        "url": format!("https://checkout.stripe.com/c/pay/{id}"),
        "_line_items": items
    });
    for key in [
        "custom_fields",
        "discounts",
        "subscription_data",
        "payment_intent_data",
        "shipping_address_collection",
    ] {
        set_any(&mut doc, params, key);
    }
    set_metadata(&mut doc, params);
    ctx.put("checkout_sessions", &id, doc.clone());
    ctx.present("checkout.session", &doc, &params.expand())
}

// --- customer portal --------------------------------------------------------------------

pub fn list_portal_configurations(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let active = params.bool("active")?;
    let default = params.bool("is_default")?;
    ctx.list(
        "billing_portal.configuration",
        "/v1/billing_portal/configurations",
        params,
        &|d| {
            active.is_none_or(|a| d["active"].as_bool().unwrap_or(true) == a)
                && default.is_none_or(|x| d["is_default"].as_bool().unwrap_or(false) == x)
        },
    )
}

pub fn get_portal_configuration(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "billing_portal.configuration", args.id(), params)
}

fn portal_fields(doc: &mut Value, params: &Params) -> Res<()> {
    for key in ["business_profile", "features", "login_page"] {
        if let Some(obj) = params.obj(key) {
            let mut current = doc
                .get(key)
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            crate::engine::merge(&mut current, &Value::Object(obj));
            doc[key] = current;
        }
    }
    set_str(doc, params, "default_return_url");
    set_str(doc, params, "name");
    set_bool(doc, params, "active")?;
    set_metadata(doc, params);
    Ok(())
}

pub fn create_portal_configuration(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let features = params
        .obj("features")
        .ok_or_else(|| StripeError::missing_param("features"))?;
    let now = ctx.now();
    let id = ctx.id("billing_portal.configuration");
    let mut doc = json!({
        "id": id,
        "object": "billing_portal.configuration",
        "active": true,
        "application": null,
        "business_profile": {"headline": null, "privacy_policy_url": null, "terms_of_service_url": null},
        "created": now,
        "default_return_url": null,
        "features": {},
        "is_default": ctx.docs("billing_portal_configurations").is_empty(),
        "login_page": {"enabled": false, "url": null},
        "metadata": {},
        "name": null,
        "updated": now
    });
    doc["features"] = Value::Object(features);
    portal_fields(&mut doc, params)?;
    ctx.put("billing_portal_configurations", &id, doc.clone());
    ctx.emit(
        "billing_portal.configuration.created",
        "billing_portal.configuration",
        &doc,
        None,
    );
    ctx.present("billing_portal.configuration", &doc, &params.expand())
}

pub fn update_portal_configuration(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let before = ctx.require("billing_portal.configuration", &id, "id")?;
    let mut doc = before.clone();
    portal_fields(&mut doc, params)?;
    doc["updated"] = json!(ctx.now());
    ctx.put("billing_portal_configurations", &id, doc.clone());
    ctx.emit(
        "billing_portal.configuration.updated",
        "billing_portal.configuration",
        &doc,
        Some(previous(&before, &doc)),
    );
    ctx.present("billing_portal.configuration", &doc, &params.expand())
}

pub fn create_portal_session(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let customer = params.req_str("customer")?;
    require_ref(ctx, "customer", &customer, "customer")?;
    let configuration = match params.str("configuration").filter(|s| !s.is_empty()) {
        Some(c) => {
            require_ref(ctx, "billing_portal.configuration", &c, "configuration")?;
            c
        }
        None => match ctx
            .docs("billing_portal_configurations")
            .into_iter()
            .find(|(_, d)| d["is_default"] == true)
        {
            Some((k, _)) => k,
            None => {
                return Err(StripeError::invalid(
                    "No configuration provided and your test mode default configuration has not been created. Provide a configuration or create your default by saving your customer portal settings in test mode at https://dashboard.stripe.com/test/settings/billing/portal.",
                ));
            }
        },
    };
    let id = ctx.id("billing_portal.session");
    let doc = json!({
        "id": id,
        "object": "billing_portal.session",
        "configuration": configuration,
        "created": ctx.now(),
        "customer": customer,
        "flow": null,
        "livemode": ctx.livemode(),
        "locale": params.str("locale"),
        "on_behalf_of": null,
        "return_url": params.str("return_url"),
        "url": format!("https://billing.stripe.com/p/session/test_{}", short_hash(&id))
    });
    ctx.put("billing_portal_sessions", &id, doc.clone());
    ctx.present("billing_portal.session", &doc, &params.expand())
}

// --- settings-style resources -----------------------------------------------------------

pub fn list_payment_method_configurations(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let active = params.bool("active")?;
    ctx.list(
        "payment_method_configuration",
        "/v1/payment_method_configurations",
        params,
        &|d| active.is_none_or(|a| d["active"].as_bool().unwrap_or(true) == a),
    )
}

pub fn get_payment_method_configuration(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "payment_method_configuration", args.id(), params)
}

pub fn list_payment_method_domains(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list(
        "payment_method_domain",
        "/v1/payment_method_domains",
        params,
        &|d| eq_param(d, "domain_name", params),
    )
}

pub fn create_payment_method_domain(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let domain = params.req_str("domain_name")?;
    let id = ctx.id("payment_method_domain");
    let status = json!({"status": "active"});
    let doc = json!({
        "id": id,
        "object": "payment_method_domain",
        "amazon_pay": status,
        "apple_pay": status,
        "created": ctx.now(),
        "domain_name": domain,
        "enabled": params.bool("enabled")?.unwrap_or(true),
        "google_pay": status,
        "klarna": status,
        "link": status,
        "paypal": status
    });
    ctx.put("payment_method_domains", &id, doc.clone());
    ctx.present("payment_method_domain", &doc, &params.expand())
}

pub fn list_webhook_endpoints(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    ctx.list("webhook_endpoint", "/v1/webhook_endpoints", params, &|_| {
        true
    })
}

pub fn get_webhook_endpoint(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    retrieve(ctx, "webhook_endpoint", args.id(), params)
}

fn enabled_events(params: &Params) -> Res<Option<Vec<String>>> {
    if !params.has("enabled_events") {
        return Ok(None);
    }
    let events: Vec<String> = params
        .list("enabled_events")
        .iter()
        .filter_map(scalar_string)
        .collect();
    if events.is_empty() {
        return Err(StripeError::missing_param("enabled_events"));
    }
    Ok(Some(events))
}

pub fn create_webhook_endpoint(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let url = params.req_str("url")?;
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(StripeError::invalid(format!("Invalid URL: {url}")).param("url"));
    }
    let events =
        enabled_events(params)?.ok_or_else(|| StripeError::missing_param("enabled_events"))?;
    let id = ctx.id("webhook_endpoint");
    let mut doc = json!({
        "id": id,
        "object": "webhook_endpoint",
        "api_version": params.str("api_version"),
        "application": null,
        "created": ctx.now(),
        "description": params.str("description"),
        "enabled_events": events,
        "metadata": {},
        "secret": format!("whsec_{}", short_hash(&id)),
        "status": "enabled",
        "url": url
    });
    set_metadata(&mut doc, params);
    ctx.put("webhook_endpoints", &id, doc.clone());
    ctx.present("webhook_endpoint", &doc, &params.expand())
}

pub fn update_webhook_endpoint(ctx: &mut Ctx, args: &Args, params: &Params) -> Res<Value> {
    let id = args.id().to_owned();
    let mut doc = ctx.require("webhook_endpoint", &id, "id")?;
    doc.as_object_mut().map(|m| m.remove("secret"));
    if let Some(events) = enabled_events(params)? {
        doc["enabled_events"] = json!(events);
    }
    set_str(&mut doc, params, "url");
    set_str(&mut doc, params, "description");
    if let Some(disabled) = params.bool("disabled")? {
        doc["status"] = json!(if disabled { "disabled" } else { "enabled" });
    }
    set_metadata(&mut doc, params);
    ctx.put("webhook_endpoints", &id, doc.clone());
    ctx.present("webhook_endpoint", &doc, &params.expand())
}

pub fn create_test_clock(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let frozen = params
        .int("frozen_time")?
        .ok_or_else(|| StripeError::missing_param("frozen_time"))?;
    let id = ctx.id("test_helpers.test_clock");
    let doc = json!({
        "id": id,
        "object": "test_helpers.test_clock",
        "created": ctx.now(),
        "deletes_after": ctx.now() + 30 * 86_400,
        "frozen_time": frozen,
        "livemode": false,
        "name": params.str("name"),
        "status": "ready",
        "status_details": {}
    });
    ctx.put("test_clocks", &id, doc.clone());
    ctx.emit(
        "test_helpers.test_clock.created",
        "test_helpers.test_clock",
        &doc,
        None,
    );
    ctx.present("test_helpers.test_clock", &doc, &params.expand())
}

/// Stripe's product tax codes (a small, fixed subset of the public list).
const TAX_CODES: &[(&str, &str, &str)] = &[
    (
        "txcd_00000000",
        "Nontaxable",
        "Any nontaxable good or service that is exempt from sales tax.",
    ),
    (
        "txcd_10000000",
        "General - Electronically Supplied Services",
        "A digital service provided mainly through the internet with minimal human involvement.",
    ),
    (
        "txcd_10103000",
        "Software as a service (SaaS) - personal use",
        "Pre-written software, delivered electronically, but access remotely.",
    ),
    (
        "txcd_10103001",
        "Software as a service (SaaS) - business use",
        "Pre-written software, delivered electronically, but access remotely, for business use.",
    ),
    (
        "txcd_20030000",
        "General - Services",
        "General category for services. It should be used only when there is no more specific services category.",
    ),
    (
        "txcd_99999999",
        "General - Tangible Goods",
        "A physical good that can be moved or touched. Also known as tangible personal property.",
    ),
];

fn tax_code(code: &(&str, &str, &str)) -> Value {
    json!({"id": code.0, "object": "tax_code", "description": code.2, "name": code.1})
}

pub fn list_tax_codes(ctx: &mut Ctx, _: &Args, params: &Params) -> Res<Value> {
    let items: Vec<Value> = TAX_CODES.iter().map(tax_code).collect();
    ctx.page("tax_code", "/v1/tax_codes", items, params)
}

pub fn get_tax_code(_: &mut Ctx, args: &Args, _: &Params) -> Res<Value> {
    TAX_CODES
        .iter()
        .find(|c| c.0 == args.id())
        .map(tax_code)
        .ok_or_else(|| StripeError::missing("tax_code", args.id(), "id"))
}

// --- products this account has not activated -------------------------------------------

fn not_activated(product: &str, url: &str) -> StripeError {
    StripeError::invalid(format!(
        "Your account is not set up to use {product}. Please visit {url} to get started."
    ))
    .status(403)
}

pub fn issuing(_: &mut Ctx, _: &Args, _: &Params) -> Res<Value> {
    Err(not_activated(
        "Issuing",
        "https://dashboard.stripe.com/issuing/overview",
    ))
}

pub fn tax(_: &mut Ctx, _: &Args, _: &Params) -> Res<Value> {
    Err(not_activated("Tax", "https://dashboard.stripe.com/tax"))
}

pub fn sigma_refusal() -> StripeError {
    not_activated("Sigma", "https://dashboard.stripe.com/sigma")
}
