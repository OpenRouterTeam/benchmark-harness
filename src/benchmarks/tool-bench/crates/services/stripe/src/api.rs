//! The operation table: every operation in the catalogue, with its REST path (Stripe's own
//! placeholder names) and handler. The MCP tools address operations by id; the REST face
//! matches method and path. Both end in [`dispatch`].

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::{
    billing, catalog, commerce,
    engine::Ctx,
    error::{Res, StripeError},
    params::Params,
    payments,
};

pub type Handler = fn(&mut Ctx, &Args, &Params) -> Res<Value>;

pub struct Route {
    pub op: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub handler: Handler,
}

/// Path parameters in template order.
#[derive(Debug, Default)]
pub struct Args(pub Vec<(&'static str, String)>);

impl Args {
    pub fn get(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    }

    /// The first path parameter (the resource id on every single-id route).
    pub fn id(&self) -> &str {
        self.0.first().map(|(_, v)| v.as_str()).unwrap_or_default()
    }
}

const fn r(op: &'static str, method: &'static str, path: &'static str, handler: Handler) -> Route {
    Route {
        op,
        method,
        path,
        handler,
    }
}

use billing as b;
use commerce as c;
use payments as p;

pub const ROUTES: &[Route] = &[
    // Account and Connect reads.
    r("GetAccounts", "GET", "/v1/accounts", c::list_accounts),
    r(
        "GetAccountsAccount",
        "GET",
        "/v1/accounts/{account}",
        c::get_account,
    ),
    r(
        "GetApplicationFees",
        "GET",
        "/v1/application_fees",
        c::list_application_fees,
    ),
    r("GetTransfers", "GET", "/v1/transfers", c::list_transfers),
    r(
        "GetTransfersTransfer",
        "GET",
        "/v1/transfers/{transfer}",
        c::get_transfer,
    ),
    // Balance.
    r("GetBalance", "GET", "/v1/balance", p::get_balance),
    r(
        "GetBalanceTransactions",
        "GET",
        "/v1/balance_transactions",
        p::list_balance_transactions,
    ),
    r(
        "GetBalanceTransactionsId",
        "GET",
        "/v1/balance_transactions/{id}",
        p::get_balance_transaction,
    ),
    r("GetPayouts", "GET", "/v1/payouts", p::list_payouts),
    r(
        "GetPayoutsPayout",
        "GET",
        "/v1/payouts/{payout}",
        p::get_payout,
    ),
    // Customers.
    r("GetCustomers", "GET", "/v1/customers", p::list_customers),
    r("PostCustomers", "POST", "/v1/customers", p::create_customer),
    r(
        "GetCustomersSearch",
        "GET",
        "/v1/customers/search",
        p::search_customers,
    ),
    r(
        "GetCustomersCustomer",
        "GET",
        "/v1/customers/{customer}",
        p::get_customer,
    ),
    r(
        "PostCustomersCustomer",
        "POST",
        "/v1/customers/{customer}",
        p::update_customer,
    ),
    r(
        "DeleteCustomersCustomer",
        "DELETE",
        "/v1/customers/{customer}",
        p::delete_customer,
    ),
    r(
        "GetCustomersCustomerBalanceTransactions",
        "GET",
        "/v1/customers/{customer}/balance_transactions",
        p::list_customer_balance_transactions,
    ),
    r(
        "GetCustomersCustomerCashBalance",
        "GET",
        "/v1/customers/{customer}/cash_balance",
        p::get_cash_balance,
    ),
    r(
        "GetCustomersCustomerCashBalanceTransactions",
        "GET",
        "/v1/customers/{customer}/cash_balance_transactions",
        p::list_cash_balance_transactions,
    ),
    r(
        "GetCustomersCustomerPaymentMethods",
        "GET",
        "/v1/customers/{customer}/payment_methods",
        p::list_customer_payment_methods,
    ),
    r(
        "GetCustomersCustomerPaymentMethodsPaymentMethod",
        "GET",
        "/v1/customers/{customer}/payment_methods/{payment_method}",
        p::get_customer_payment_method,
    ),
    // Payments.
    r(
        "GetPaymentMethods",
        "GET",
        "/v1/payment_methods",
        p::list_payment_methods,
    ),
    r(
        "GetPaymentMethodsPaymentMethod",
        "GET",
        "/v1/payment_methods/{payment_method}",
        p::get_payment_method,
    ),
    r(
        "GetPaymentIntents",
        "GET",
        "/v1/payment_intents",
        p::list_payment_intents,
    ),
    r(
        "GetPaymentIntentsSearch",
        "GET",
        "/v1/payment_intents/search",
        p::search_payment_intents,
    ),
    r(
        "GetPaymentIntentsIntent",
        "GET",
        "/v1/payment_intents/{intent}",
        p::get_payment_intent,
    ),
    r("GetCharges", "GET", "/v1/charges", p::list_charges),
    r(
        "GetChargesSearch",
        "GET",
        "/v1/charges/search",
        p::search_charges,
    ),
    r(
        "GetChargesCharge",
        "GET",
        "/v1/charges/{charge}",
        p::get_charge,
    ),
    r(
        "GetChargesChargeRefunds",
        "GET",
        "/v1/charges/{charge}/refunds",
        p::list_charge_refunds,
    ),
    r("GetRefunds", "GET", "/v1/refunds", p::list_refunds),
    r("PostRefunds", "POST", "/v1/refunds", p::create_refund),
    r(
        "GetRefundsRefund",
        "GET",
        "/v1/refunds/{refund}",
        p::get_refund,
    ),
    r("GetDisputes", "GET", "/v1/disputes", p::list_disputes),
    r(
        "GetDisputesDispute",
        "GET",
        "/v1/disputes/{dispute}",
        p::get_dispute,
    ),
    r(
        "PostDisputesDispute",
        "POST",
        "/v1/disputes/{dispute}",
        p::update_dispute,
    ),
    r(
        "GetSetupIntents",
        "GET",
        "/v1/setup_intents",
        c::list_setup_intents,
    ),
    r(
        "GetSetupIntentsIntent",
        "GET",
        "/v1/setup_intents/{intent}",
        c::get_setup_intent,
    ),
    r("GetEvents", "GET", "/v1/events", p::list_events),
    r("GetEventsId", "GET", "/v1/events/{id}", p::get_event),
    // Catalog.
    r("GetProducts", "GET", "/v1/products", b::list_products),
    r("PostProducts", "POST", "/v1/products", b::create_product),
    r(
        "GetProductsSearch",
        "GET",
        "/v1/products/search",
        b::search_products,
    ),
    r("GetProductsId", "GET", "/v1/products/{id}", b::get_product),
    r(
        "PostProductsId",
        "POST",
        "/v1/products/{id}",
        b::update_product,
    ),
    r("GetPrices", "GET", "/v1/prices", b::list_prices),
    r("PostPrices", "POST", "/v1/prices", b::create_price),
    r(
        "GetPricesSearch",
        "GET",
        "/v1/prices/search",
        b::search_prices,
    ),
    r("GetPricesPrice", "GET", "/v1/prices/{price}", b::get_price),
    r(
        "PostPricesPrice",
        "POST",
        "/v1/prices/{price}",
        b::update_price,
    ),
    r("GetCoupons", "GET", "/v1/coupons", b::list_coupons),
    r("PostCoupons", "POST", "/v1/coupons", b::create_coupon),
    r(
        "GetCouponsCoupon",
        "GET",
        "/v1/coupons/{coupon}",
        b::get_coupon,
    ),
    r(
        "PostCouponsCoupon",
        "POST",
        "/v1/coupons/{coupon}",
        b::update_coupon,
    ),
    r(
        "DeleteCouponsCoupon",
        "DELETE",
        "/v1/coupons/{coupon}",
        b::delete_coupon,
    ),
    r(
        "GetPromotionCodes",
        "GET",
        "/v1/promotion_codes",
        b::list_promotion_codes,
    ),
    r(
        "PostPromotionCodes",
        "POST",
        "/v1/promotion_codes",
        b::create_promotion_code,
    ),
    r(
        "GetPromotionCodesPromotionCode",
        "GET",
        "/v1/promotion_codes/{promotion_code}",
        b::get_promotion_code,
    ),
    // Billing.
    r(
        "GetSubscriptions",
        "GET",
        "/v1/subscriptions",
        b::list_subscriptions,
    ),
    r(
        "PostSubscriptions",
        "POST",
        "/v1/subscriptions",
        b::create_subscription,
    ),
    r(
        "GetSubscriptionsSearch",
        "GET",
        "/v1/subscriptions/search",
        b::search_subscriptions,
    ),
    r(
        "GetSubscriptionsSubscriptionExposedId",
        "GET",
        "/v1/subscriptions/{subscription_exposed_id}",
        b::get_subscription,
    ),
    r(
        "PostSubscriptionsSubscriptionExposedId",
        "POST",
        "/v1/subscriptions/{subscription_exposed_id}",
        b::update_subscription,
    ),
    r(
        "DeleteSubscriptionsSubscriptionExposedId",
        "DELETE",
        "/v1/subscriptions/{subscription_exposed_id}",
        b::cancel_subscription,
    ),
    r(
        "GetSubscriptionItems",
        "GET",
        "/v1/subscription_items",
        b::list_subscription_items,
    ),
    r(
        "GetSubscriptionSchedules",
        "GET",
        "/v1/subscription_schedules",
        c::list_subscription_schedules,
    ),
    r(
        "GetSubscriptionSchedulesSchedule",
        "GET",
        "/v1/subscription_schedules/{schedule}",
        c::get_subscription_schedule,
    ),
    r("GetInvoices", "GET", "/v1/invoices", b::list_invoices),
    r("PostInvoices", "POST", "/v1/invoices", b::create_invoice),
    r(
        "PostInvoicesCreatePreview",
        "POST",
        "/v1/invoices/create_preview",
        b::create_preview,
    ),
    r(
        "GetInvoicesSearch",
        "GET",
        "/v1/invoices/search",
        b::search_invoices,
    ),
    r(
        "GetInvoicesInvoice",
        "GET",
        "/v1/invoices/{invoice}",
        b::get_invoice,
    ),
    r(
        "PostInvoicesInvoice",
        "POST",
        "/v1/invoices/{invoice}",
        b::update_invoice,
    ),
    r(
        "GetInvoicesInvoiceLines",
        "GET",
        "/v1/invoices/{invoice}/lines",
        b::list_invoice_lines,
    ),
    r(
        "PostInvoicesInvoiceFinalize",
        "POST",
        "/v1/invoices/{invoice}/finalize",
        b::finalize_invoice,
    ),
    r(
        "PostInvoicesInvoiceMarkUncollectible",
        "POST",
        "/v1/invoices/{invoice}/mark_uncollectible",
        b::mark_uncollectible,
    ),
    r(
        "PostInvoicesInvoiceVoid",
        "POST",
        "/v1/invoices/{invoice}/void",
        b::void_invoice,
    ),
    r(
        "GetInvoicePayments",
        "GET",
        "/v1/invoice_payments",
        b::list_invoice_payments,
    ),
    r(
        "GetInvoiceitems",
        "GET",
        "/v1/invoiceitems",
        b::list_invoiceitems,
    ),
    r(
        "PostInvoiceitems",
        "POST",
        "/v1/invoiceitems",
        b::create_invoiceitem,
    ),
    r(
        "GetInvoiceitemsInvoiceitem",
        "GET",
        "/v1/invoiceitems/{invoiceitem}",
        b::get_invoiceitem,
    ),
    r(
        "PostInvoiceitemsInvoiceitem",
        "POST",
        "/v1/invoiceitems/{invoiceitem}",
        b::update_invoiceitem,
    ),
    r(
        "GetCreditNotes",
        "GET",
        "/v1/credit_notes",
        c::list_credit_notes,
    ),
    // Checkout, Payment Links, customer portal.
    r(
        "GetPaymentLinks",
        "GET",
        "/v1/payment_links",
        c::list_payment_links,
    ),
    r(
        "PostPaymentLinks",
        "POST",
        "/v1/payment_links",
        c::create_payment_link,
    ),
    r(
        "GetPaymentLinksPaymentLink",
        "GET",
        "/v1/payment_links/{payment_link}",
        c::get_payment_link,
    ),
    r(
        "PostPaymentLinksPaymentLink",
        "POST",
        "/v1/payment_links/{payment_link}",
        c::update_payment_link,
    ),
    r(
        "GetPaymentLinksPaymentLinkLineItems",
        "GET",
        "/v1/payment_links/{payment_link}/line_items",
        c::payment_link_line_items,
    ),
    r(
        "GetCheckoutSessions",
        "GET",
        "/v1/checkout/sessions",
        c::list_checkout_sessions,
    ),
    r(
        "PostCheckoutSessions",
        "POST",
        "/v1/checkout/sessions",
        c::create_checkout_session,
    ),
    r(
        "GetCheckoutSessionsSession",
        "GET",
        "/v1/checkout/sessions/{session}",
        c::get_checkout_session,
    ),
    r(
        "GetCheckoutSessionsSessionLineItems",
        "GET",
        "/v1/checkout/sessions/{session}/line_items",
        c::checkout_session_line_items,
    ),
    r(
        "GetBillingPortalConfigurations",
        "GET",
        "/v1/billing_portal/configurations",
        c::list_portal_configurations,
    ),
    r(
        "PostBillingPortalConfigurations",
        "POST",
        "/v1/billing_portal/configurations",
        c::create_portal_configuration,
    ),
    r(
        "GetBillingPortalConfigurationsConfiguration",
        "GET",
        "/v1/billing_portal/configurations/{configuration}",
        c::get_portal_configuration,
    ),
    r(
        "PostBillingPortalConfigurationsConfiguration",
        "POST",
        "/v1/billing_portal/configurations/{configuration}",
        c::update_portal_configuration,
    ),
    r(
        "PostBillingPortalSessions",
        "POST",
        "/v1/billing_portal/sessions",
        c::create_portal_session,
    ),
    // Settings-style resources.
    r(
        "GetPaymentMethodConfigurations",
        "GET",
        "/v1/payment_method_configurations",
        c::list_payment_method_configurations,
    ),
    r(
        "GetPaymentMethodConfigurationsConfiguration",
        "GET",
        "/v1/payment_method_configurations/{configuration}",
        c::get_payment_method_configuration,
    ),
    r(
        "GetPaymentMethodDomains",
        "GET",
        "/v1/payment_method_domains",
        c::list_payment_method_domains,
    ),
    r(
        "PostPaymentMethodDomains",
        "POST",
        "/v1/payment_method_domains",
        c::create_payment_method_domain,
    ),
    r(
        "GetWebhookEndpoints",
        "GET",
        "/v1/webhook_endpoints",
        c::list_webhook_endpoints,
    ),
    r(
        "PostWebhookEndpoints",
        "POST",
        "/v1/webhook_endpoints",
        c::create_webhook_endpoint,
    ),
    r(
        "GetWebhookEndpointsWebhookEndpoint",
        "GET",
        "/v1/webhook_endpoints/{webhook_endpoint}",
        c::get_webhook_endpoint,
    ),
    r(
        "PostWebhookEndpointsWebhookEndpoint",
        "POST",
        "/v1/webhook_endpoints/{webhook_endpoint}",
        c::update_webhook_endpoint,
    ),
    r(
        "PostTestHelpersTestClocks",
        "POST",
        "/v1/test_helpers/test_clocks",
        c::create_test_clock,
    ),
    r("GetTaxCodes", "GET", "/v1/tax_codes", c::list_tax_codes),
    r(
        "GetTaxCodesId",
        "GET",
        "/v1/tax_codes/{id}",
        c::get_tax_code,
    ),
    // Products this account has not activated.
    r(
        "GetIssuingAuthorizations",
        "GET",
        "/v1/issuing/authorizations",
        c::issuing,
    ),
    r(
        "GetIssuingAuthorizationsAuthorization",
        "GET",
        "/v1/issuing/authorizations/{authorization}",
        c::issuing,
    ),
    r(
        "GetIssuingCardholders",
        "GET",
        "/v1/issuing/cardholders",
        c::issuing,
    ),
    r(
        "GetIssuingCardholdersCardholder",
        "GET",
        "/v1/issuing/cardholders/{cardholder}",
        c::issuing,
    ),
    r("GetIssuingCards", "GET", "/v1/issuing/cards", c::issuing),
    r(
        "GetIssuingCardsCard",
        "GET",
        "/v1/issuing/cards/{card}",
        c::issuing,
    ),
    r(
        "GetIssuingDisputes",
        "GET",
        "/v1/issuing/disputes",
        c::issuing,
    ),
    r(
        "GetIssuingDisputesDispute",
        "GET",
        "/v1/issuing/disputes/{dispute}",
        c::issuing,
    ),
    r(
        "GetIssuingTransactions",
        "GET",
        "/v1/issuing/transactions",
        c::issuing,
    ),
    r(
        "GetIssuingTransactionsTransaction",
        "GET",
        "/v1/issuing/transactions/{transaction}",
        c::issuing,
    ),
    r(
        "GetTaxCalculationsCalculation",
        "GET",
        "/v1/tax/calculations/{calculation}",
        c::tax,
    ),
    r(
        "GetTaxCalculationsCalculationLineItems",
        "GET",
        "/v1/tax/calculations/{calculation}/line_items",
        c::tax,
    ),
    r(
        "PostTaxCalculations",
        "POST",
        "/v1/tax/calculations",
        c::tax,
    ),
    r(
        "GetTaxRegistrations",
        "GET",
        "/v1/tax/registrations",
        c::tax,
    ),
    r(
        "PostTaxRegistrations",
        "POST",
        "/v1/tax/registrations",
        c::tax,
    ),
    r(
        "GetTaxRegistrationsId",
        "GET",
        "/v1/tax/registrations/{id}",
        c::tax,
    ),
    r("GetTaxSettings", "GET", "/v1/tax/settings", c::tax),
    r("PostTaxSettings", "POST", "/v1/tax/settings", c::tax),
];

pub fn route(op: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.op == op)
}

fn placeholders(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .collect()
}

/// The route for a REST request, preferring literal segments (`/v1/customers/search`) over
/// placeholders (`/v1/customers/{customer}`).
pub fn match_path(method: &str, path: &str) -> Option<(&'static Route, Args)> {
    let segs: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let mut best: Option<(usize, &'static Route, Args)> = None;
    for route in ROUTES.iter().filter(|r| r.method == method) {
        let tmpl: Vec<&'static str> = route.path.split('/').collect();
        if tmpl.len() != segs.len() {
            continue;
        }
        let mut args = Vec::new();
        let mut literals = 0;
        let ok = tmpl.iter().zip(&segs).all(|(t, s)| {
            if let Some(name) = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                args.push((name, crate::params::percent_decode(s)));
                !s.is_empty()
            } else {
                literals += 1;
                t == s
            }
        });
        if ok && best.as_ref().is_none_or(|(l, _, _)| literals > *l) {
            best = Some((literals, route, Args(args)));
        }
    }
    best.map(|(_, route, args)| (route, args))
}

/// Takes the path parameters out of an MCP `parameters` object. The real server names every
/// placeholder `{id}`, so `id` is accepted alongside Stripe's own names.
pub fn args_from_params(route: &Route, params: &mut Params) -> Res<Args> {
    let mut args = Vec::new();
    for name in placeholders(route.path) {
        let value = params
            .remove(name)
            .filter(|v| !v.is_null())
            .or_else(|| params.remove("id").filter(|v| !v.is_null()));
        match value.as_ref().and_then(crate::params::scalar_string) {
            Some(v) if !v.is_empty() => args.push((name, v)),
            _ => return Err(StripeError::missing_param("id")),
        }
    }
    Ok(Args(args))
}

/// Rejects parameters the operation does not take, as Stripe does.
pub fn check_params(op: &str, params: &Params) -> Res<()> {
    let Some(entry) = catalog::find(op) else {
        return Ok(());
    };
    let allowed: BTreeSet<String> = entry.param_names();
    for key in params.keys() {
        if !allowed.contains(key) {
            return Err(StripeError::unknown_param(key));
        }
    }
    Ok(())
}

pub fn dispatch(ctx: &mut Ctx, route: &Route, args: &Args, params: &Params) -> Res<Value> {
    check_params(route.op, params)?;
    (route.handler)(ctx, args, params)
}

/// `{"deleted": true}` stub returned by deletes.
pub fn deleted(id: &str, object: &str) -> Value {
    json!({"id": id, "object": object, "deleted": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogued_operation_has_a_route_and_vice_versa() {
        for op in catalog::operations() {
            let route = route(&op.id).unwrap_or_else(|| panic!("no route for {}", op.id));
            assert_eq!(route.method, op.method, "{}", op.id);
            let normalized: String = route
                .path
                .split('/')
                .map(|s| if s.starts_with('{') { "{id}" } else { s })
                .collect::<Vec<_>>()
                .join("/");
            assert_eq!(normalized, op.path, "{}", op.id);
        }
        for route in ROUTES {
            assert!(
                catalog::find(route.op).is_some() || route.op == "DeleteCustomersCustomer",
                "{} not in catalogue",
                route.op
            );
        }
    }

    #[test]
    fn literal_segments_win() {
        let (route, _) = match_path("GET", "/v1/customers/search").unwrap();
        assert_eq!(route.op, "GetCustomersSearch");
        let (route, args) = match_path("GET", "/v1/customers/cus_123").unwrap();
        assert_eq!(route.op, "GetCustomersCustomer");
        assert_eq!(args.id(), "cus_123");
        assert!(match_path("GET", "/v1/nope").is_none());
    }
}
