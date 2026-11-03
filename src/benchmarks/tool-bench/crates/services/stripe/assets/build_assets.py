"""Regenerates the pinned Stripe assets of mock-stripe from Kiln-AI/stripe_world (MIT).

    git clone https://github.com/Kiln-AI/stripe_world /tmp/stripe_world
    python3 -I crates/services/stripe/assets/build_assets.py /tmp/stripe_world crates/services/stripe/assets

Outputs (compact JSON, keys sorted, so reruns are byte-identical):
  operations.json  the operations the remote Stripe MCP catalogues, as its stripe_api_details
                   documents (stripe_world spec/discovery_index.json, API 2026-08-26.dahlia),
                   plus GetEvents/GetEventsId built from spec3.json with "mcp": false (served
                   on the REST face only: the remote MCP does not expose v1 events).
  objects.json     per object type: the nullable fields (emitted as null when unset) and the
                   expandable fields, from Stripe's OpenAPI spec3.json.
"""

import json
import sys
from pathlib import Path

OBJECTS = [
    "account", "application_fee", "balance", "balance_transaction",
    "billing_portal.configuration", "billing_portal.session", "cash_balance", "charge",
    "checkout.session", "coupon", "credit_note", "customer", "customer_balance_transaction",
    "customer_cash_balance_transaction", "discount", "dispute", "event", "invoice",
    "invoice_payment", "invoiceitem", "issuing.authorization", "issuing.card",
    "issuing.cardholder", "issuing.dispute", "issuing.transaction", "line_item", "item",
    "payment_intent", "payment_link", "payment_method", "payment_method_configuration",
    "payment_method_domain", "payout", "price", "product", "promotion_code", "refund",
    "setup_intent", "subscription", "subscription_item", "subscription_schedule", "tax_code",
    "test_helpers.test_clock", "transfer", "webhook_endpoint",
]


def param_doc(schema, description, required):
    kind = schema.get("type")
    if kind is None and "anyOf" in schema:
        kinds = []
        for option in schema["anyOf"]:
            t = option.get("type", "string")
            if t not in kinds:
                kinds.append(t)
        kind = " | ".join(kinds)
    doc = {"description": description or "", "required": required, "type": kind or "string"}
    if "enum" in schema:
        doc["enum"] = schema["enum"]
    if kind == "array" and "items" in schema:
        doc["items"] = param_doc(schema["items"], schema["items"].get("description", ""), False)
    return doc


def rest_only(spec, op_id, path):
    op = spec["paths"][path]["get"]
    params = {"body": {}, "path": {}, "query": {}}
    for p in op.get("parameters", []):
        params[p["in"]][p["name"]] = param_doc(
            p["schema"], p.get("description", ""), p.get("required", False)
        )
    return {
        "description": op.get("description", "").replace("<p>", "").replace("</p>", ""),
        "id": op_id,
        "keywords": ["v1", "events", "get", "retrieve" if "{" in path else "list", "event"],
        "mcp": False,
        "method": "GET",
        "openapi_spec_version": "2026-08-26.dahlia",
        "parameters": params,
        "path": path,
        "required_permissions": ["event_read"],
        "summary": op.get("summary", ""),
        "tags": ["event"],
    }


def main():
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    spec_dir = src / "src/seahaven_stripe_world/spec"
    index = json.loads((spec_dir / "discovery_index.json").read_text())
    spec = json.loads((spec_dir / "spec3.json").read_text())
    ops = [index[k] for k in sorted(index)]
    ops.append(rest_only(spec, "GetEvents", "/v1/events"))
    ops.append(rest_only(spec, "GetEventsId", "/v1/events/{id}"))
    ops.sort(key=lambda o: o["id"])
    (out / "operations.json").write_text(
        json.dumps(ops, sort_keys=True, separators=(",", ":")) + "\n"
    )

    schemas = spec["components"]["schemas"]
    objects = {}
    for name in OBJECTS:
        schema = schemas[name]
        props = schema.get("properties", {})
        objects[name] = {
            "expandable": sorted(schema.get("x-expandableFields", [])),
            "nullable": sorted(k for k, v in props.items() if v.get("nullable")),
        }
    (out / "objects.json").write_text(
        json.dumps(objects, sort_keys=True, separators=(",", ":")) + "\n"
    )


main()
