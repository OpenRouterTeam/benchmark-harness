# Provenance and licences

Everything under `scenarios/` (instructions, datasets, expectations) and every mock in
`crates/services/` is written by OpenRouter for this benchmark and released under this
repository's licence. No customer data or real account data is included: names, IDs, tokens and
records in the datasets are invented.

Upstream material vendored into the mocks or the tools image:

| Path | Source | Licence |
|---|---|---|
| `crates/services/cloudflare/assets/spec.json` | Trimmed from [cloudflare/mcp](https://github.com/cloudflare/mcp) `src/generated/spec/spec.json.txt` at `68c7ec0`, itself generated from [cloudflare/api-schemas](https://github.com/cloudflare/api-schemas) (`trim-spec.py`) | BSD-3-Clause (api-schemas) |
| `crates/services/stripe/assets/operations.json`, `objects.json` | Built from [Kiln-AI/stripe_world](https://github.com/Kiln-AI/stripe_world) and Stripe's [OpenAPI spec](https://github.com/stripe/openapi) (`build_assets.py`) | MIT (both) |
| `crates/services/aws/assets/service-reference*` | Copies of the public [AWS service reference](https://docs.aws.amazon.com/service-authorization/latest/reference/service-reference.html) documents served at `servicereference.us-east-1.amazonaws.com` | AWS documentation; no licence file upstream |
| `servers/snapshots/*.json` | Tool lists captured from each pinned MCP server in `catalog/services.toml` (`deploy/capture-stack.sh`) | Each server's own licence (see its pin in the catalog) |
| `servers/npm`, `servers/python` | Lockfiles for the pinned MCP servers; the packages are installed into the tools image at build time, not vendored | Each package's own licence |
