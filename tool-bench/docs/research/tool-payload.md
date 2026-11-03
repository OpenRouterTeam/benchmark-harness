# Measured tool payload (2026-10-30)

Captured with `deploy/capture-stack.sh`: world, tools, and toolmeter on an `--internal` Docker
network (no internet). Every endpoint is reached exactly as an agent reaches it, through
`tools-supervisor` (bridge or replica) over streamable HTTP. Two consecutive runs produced
identical hashes for all 32 endpoints (re-verified 2026-10-31 on the fully merged tree).

## Totals

- **32 endpoints, 1595 tools**
- **726,967 o200k tokens** for the compact JSON of every `tools/list` entry (2,910,129 bytes)
- Without `outputSchema`: 2,290,797 bytes, about 572,254 tokens at the same
  bytes-per-token rate. 14 endpoints send output schemas: atlassian, aws-api, aws-cloudwatch, aws-iam, brave-search, cloudflare, firecrawl, google-workspace, kubernetes, linear, mongodb, okta, pagerduty, sentry.

Both figures are well above the 260k-token floor. What a model actually receives depends on the
harness. It adds its own system prompt and built-in tools, reformats MCP tools into its provider's
schema, and may or may not forward `outputSchema`. The per-trial measurement on the router side
(spec section 8) is the number to compare against context windows. o200k is a provider-neutral
estimate; Anthropic `count_tokens` was not used because no API key is configured here.

## Per endpoint

| Endpoint | Server | Tools | Bytes | o200k tokens | Flags | sha256 |
|---|---|---|---|---|---|---|
| argocd | argocd-mcp 0.0.0 | 16 | 22435 | 4969 | 16 | `f6a7badce258` |
| atlassian | Atlassian MCP 3.4.11 | 98 | 115008 | 27576 | 0 | `ab4bee816991` |
| aws-api | AWS-API-MCP 3.4.11 | 2 | 7995 | 1829 | 0 | `ee4fe02c9797` |
| aws-cloudwatch | awslabs.cloudwatch-mcp-server  | 19 | 90715 | 20472 | 0 | `249e71595de6` |
| aws-iam | awslabs.iam-mcp-server  | 29 | 63640 | 14619 | 0 | `93488f7f87fd` |
| brave-search | brave-search-mcp-server 2.1.4 | 8 | 110731 | 25500 | 0 | `fed4dcff1fa9` |
| cloudflare | cloudflare-api 0.1.0 | 4 | 9588 | 2343 | 0 | `6bedbe079e3f` |
| datadog | Datadog MCP Server 1.8.0 | 21 | 8608 | 1965 | 0 | `15d59c703efc` |
| elasticsearch | rmcp 0.2.1 | 5 | 2271 | 500 | 5 | `25c7c86a39c7` |
| exa | exa-search-server 3.4.2 | 12 | 17285 | 3841 | 12 | `ec0f64018a5b` |
| firecrawl | firecrawl-fastmcp 3.36.0 | 28 | 96766 | 21763 | 28 | `5cf8310b4c0b` |
| github | github-mcp-server v2.0.3 | 91 | 259600 | 116619 | 0 | `2e11422550e4` |
| gitlab | zereight-gitlab-mcp-server 2.3.16 | 285 | 298478 | 66004 | 0 | `8679ae1d5176` |
| google-workspace | google_workspace 4.1.4 | 119 | 248638 | 57002 | 0 | `c16a44830ee6` |
| grafana | mcp-grafana v2.1.1 | 79 | 112867 | 25631 | 0 | `d4379b55347e` |
| hubspot | hubspot-mcp-server 0.4.0 | 21 | 40375 | 9158 | 21 | `3389dd469cfc` |
| kubernetes | kubernetes-mcp-server v0.1.1 | 74 | 145736 | 34133 | 0 | `999adfa5ca94` |
| linear | linear 1.4.5 | 198 | 242328 | 57929 | 0 | `60405652c029` |
| mongodb | MongoDB MCP Server 3.2.2 | 49 | 143243 | 33483 | 0 | `cf58502f39d3` |
| notion | Notion API 1.0.0 | 24 | 76215 | 17500 | 0 | `f9544e7c8498` |
| okta | Okta IDaaS MCP Server 1.30.0 | 113 | 148575 | 34845 | 0 | `e9de8e23b922` |
| pagerduty | PagerDuty MCP Server 1.30.0 | 101 | 397093 | 91643 | 0 | `fa12e2fccc51` |
| salesforce | salesforce-mcp-server 0.0.8 | 15 | 25165 | 5893 | 0 | `1e480a90363e` |
| sentry | Sentry MCP 0.42.0 | 9 | 26332 | 6143 | 9 | `49d56681276c` |
| shopify | shopify 1.0.0 | 14 | 10156 | 2461 | 14 | `3411400dc4f8` |
| slack | mcp.slack.com captured 2026-10-30 | 27 | 77057 | 18265 | 0 | `63d50caba9fe` |
| stripe | mcp.stripe.com captured 2026-09-22 | 10 | 11946 | 2467 | 10 | `5678778aa000` |
| supabase | supabase 0.15.0 | 36 | 27047 | 6394 | 36 | `a4906781585b` |
| tavily | tavily-mcp 0.2.22 | 5 | 7694 | 1653 | 0 | `b2bcd45339fe` |
| terraform | terraform-mcp-server 1.4.1 | 61 | 51616 | 11150 | 0 | `6a2817b99008` |
| vercel | vercel 0.1.0 | 15 | 11736 | 2502 | 0 | `83aff2292433` |
| zendesk | Zendesk 0.1.0 | 7 | 3190 | 715 | 0 | `c4b6e9a68321` |
| **Total (32 endpoints)** | | **1595** | **2910129** | **726967** | | |

## Differences from the research estimates

- **sentry: 9 tools, not 14.** Its LLM-backed search tools register only when an LLM API key is set.
  We leave the keys unset (spec section 4, determinism).
- **shopify: 14 tools.** The pinned npm release 1.0.8 has 14; the 44 in the research came from the
  repository's unreleased `main`.
- **tavily: 0.2.22.** 0.2.23 was never published to npm.
- **linear: 198 tools.** Interim community schemas until the official `mcp.linear.app` capture (D5).
- **github, pagerduty, brave-search, aws-cloudwatch** are the largest. Most of their size is
  `outputSchema`.

## Startup handshakes the world answers (P3)

| Server | Needs at startup | World answer |
|---|---|---|
| terraform-mcp-server | `GET /api/v2/ping` (else TFE tools are not registered) | `mock-terraform` |
| okta-mcp-server | OAuth device flow on `octo-corp.okta.com` | `mock-okta` (JWT, `exp` 2100) |
| aws-api-mcp-server | service reference index from `servicereference.us-east-1.amazonaws.com` | `mock-aws`, pinned copy |
| mcp-server-supabase | Content API `{ schema }` for the `search_docs` description | `mock-supabase`, pinned SDL |
| okta-mcp-server | a keyring that never prompts | `PYTHON_KEYRING_BACKEND` = plaintext file |

## Tool lists that depend on the world

- kubernetes-mcp-server's Kiali tools list OpenShift permissions only when API discovery reports `apps.openshift.io`. No scenario dataset adds OpenShift groups, so the list is identical in every trial; keep it that way (the hash recorded per trial would catch it).
