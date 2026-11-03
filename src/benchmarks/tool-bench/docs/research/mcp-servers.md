# MCP server research (2026-10-30)

Token figures are source chars/4 estimates unless noted. Mode: 1 = official server -> mock API, 2 = our mock MCP (copied schemas) -> mock API, 3 = mock MCP only.

| Service | Server (pinned) | Transport | Base URL override | Tools | ~Tokens | Mode |
|---|---|---|---|---|---|---|
| Zendesk | none official; community reminia/zendesk-mcp-server @806d1ca | stdio | partial: ZENPY_FORCE_NETLOC/SCHEME; get_tickets hardcodes host | 7 | 1.5k | 2 |
| Notion | @notionhq/notion-mcp-server 2.6.1 @cf16e5c (local; may be sunset) | stdio / http | BASE_URL (source only, no /v1) | 24 | 20k | 1 |
| HubSpot | @hubspot/mcp-server 0.4.0 | stdio | BASE_URL_OVERRIDE | 21 | 11-13k | 1 |
| Salesforce | vendor DX server is dev tooling; use tsmztech/mcp-server-salesforce 0.0.8 @18f6500 | stdio | SALESFORCE_INSTANCE_URL (Access_Token mode) | 15 | 7k | 1 (community) |
| Stripe | current @stripe/mcp 0.3.3 is relay to hosted mcp.stripe.com; legacy 0.2.4 local | remote / legacy stdio | none (stripe-node supports host but not passed) | 11-15 remote / 22 legacy | 3-5k / 6k | 2 (serve captured remote schemas; ref Kiln-AI/stripe_world) |
| Shopify | no official Admin MCP; community GeLi2001/shopify-mcp 1.0.8 @c90faaf4 | stdio | MYSHOPIFY_DOMAIN (HTTPS forced -> TLS mock) | 44 | 7-9k | 1 (community, GraphQL mock) or 2 |
| Jira | official remote-only; community sooperset/mcp-atlassian v0.23.1 @0a5d242 | stdio / http | JIRA_URL (localhost => Server/DC mode, /rest/api/2; Cloud needs *.atlassian.net host+TLS) | 63 Jira (+35 Confluence) | 22-30k (all 98: ~36-45k) | 1 (community) |
| Google Workspace | official remote-only; community taylorwilsdon/google_workspace_mcp v2.2.1 @b28e1bc | stdio / http | none (hardcoded googleapis.com; DNS+TLS hack) ; alt nspady/google-calendar-mcp GOOGLE_CALENDAR_API_BASE_URL | 21 Gmail+Cal (112 all) | 17-25k (all ~60-70k) | 2 (copy schemas) |
| Slack | official remote mcp.slack.com (27 tools, ~18k tok, capturable); community korotovsky/slack-mcp-server v1.3.0 @b88c0de3 | remote / stdio,http | korotovsky: none, but uses url from auth.test response; or SLACK_MCP_PROXY+SLACK_MCP_SERVER_CA | 27 official / 22 community | 18k / 5-6k | 2 (serve captured official schemas) |
| Linear | official remote mcp.linear.app (~31 tools UNVERIFIED); community tacticlaunch/mcp-linear 1.4.5 @d5b4a9d4 | remote / stdio | none (SDK accepts apiUrl; needs fork or proxy) | 198 community / ~31 official | ~35-50k / 6-10k | 2, or forked community (GraphQL mock) |
| GitHub | github/github-mcp-server v2.0.3 @f2a07cd (vendor) | stdio/http/remote | GITHUB_HOST=http://localhost:PORT (GHES layout: /api/v3, /api/graphql) | 45 default / 93 all | 12k default, 26k all (x2 w/ outputSchema) | 1 |
| GitLab | vendor built into GitLab (58 tools); community zereight/gitlab-mcp v2.3.16 @c352ecb | stdio/http | GITLAB_API_URL, GITLAB_GRAPHQL_URL | 40 default / 276 all | 13k / 71k | 1 (community) |
| Firecrawl | firecrawl-mcp 3.36.0 @b59c48a (vendor) | stdio/http | FIRECRAWL_API_URL | 28 | 15k (25k w/ outputSchema) | 1 |
| Brave Search | brave/brave-search-mcp-server v2.1.4 @ee06f81 (vendor) | stdio/http | none (hardcoded src/BraveAPI/index.ts:52) | 8 | 6-9k | 1 w/ patch/proxy, or 2 |
| Exa | exa-mcp-server 3.4.2 @55cc85c (vendor) | stdio | EXA_API_BASE_URL | 3 default / 12 all | 2-4k | 1 |
| Tavily | tavily-mcp 0.2.23 @3be0c42 (vendor) | stdio | none (hardcoded; axios honours HTTPS_PROXY) | 6 | 3.7k | 1 via proxy |
| ArgoCD | argocd-mcp v0.9.0 @fbaae3a (argoproj-labs) | stdio/sse/http | ARGOCD_BASE_URL | 16 | 2.5k | 1 |
| HCP Terraform | hashicorp/terraform-mcp-server v1.4.1 @062700d (vendor) | stdio/http | TFE_ADDRESS (registry hardcoded) | 9 default / 61 all | 2k / 10k | 1 (TFE tools; JSON:API) |
| Cloudflare | cloudflare/mcp @68c7ec0 (Code Mode, 4 tools) | Workers (wrangler dev) | CLOUDFLARE_API_BASE | 4 (~2,500 non-codemode) | 2k (~925k direct!) | 2 (curated subset) |
| Vercel | official remote-only closed (248 tools); community nganiet/mcp-vercel stale | remote | none | 248 / 15 | ? / 4k | 2 |

## Notes
- Notion: Notion-Version header 2025-09-03 and 2026-03-11 (markdown tools); all $defs embedded per tool. Remote server (68 tools) not mockable.
- Zendesk: use API-token mode; mock Zenpy shapes; /api/v2 tickets, comments, help_center.
- HubSpot: plain fetch; endpoints crm/v3 objects/properties, crm/v4 associations, engagements/v1, automation/v4 flows, account-info. Defs ~16 months old vs remote.
- Salesforce: tsmztech admin tools need Metadata SOAP (skip/limit those). Mock must 200 any token. Endpoints: /services/data, query, search, sobjects describe/CRUD, tooling.
- Stripe: remote tools are generic (stripe_api_read/write by OpenAPI operationId). Dec 2026 agent-key requirement. Legacy needs patch for host.
- Shopify: Admin GraphQL single endpoint -> mock must answer GraphQL docs. GeLi2001 omits tool descriptions in tools/list. Dev MCP is docs-only; UCP MCP (8 tools) hosted per store.
- Jira: toolsets via TOOLSETS/ENABLED_TOOLS/READ_ONLY_MODE. Mock Server/DC (api/2) is simplest. Endpoints: issue CRUD/transitions/comments/worklog, search, project, field, user, agile board/sprint, servicedeskapi.
- Google: gmail/v1 messages/threads/labels/drafts/filters + /batch/gmail/v1 multipart; calendar/v3 calendarList/events/freeBusy; oauth2/v2/userinfo. Official remote: Gmail 11, Calendar 9 tools.
- Slack: official descriptions embed caller user_id -> freeze on capture. canvas tools ~10k chars each. korotovsky needs Web API (auth.test, conversations.*, chat.postMessage, search.messages, users.*, reactions.*, usergroups.*); edge/client APIs only for xoxc.
- Linear: GraphQL only (POST /graphql). SDK v100+ rejects http:// unless localhost. tacticlaunch 198 tools always on -> big token block.
- CROSS-CUTTING: an HTTPS-intercepting proxy with custom CA (HTTPS_PROXY + CA env) can redirect hardcoded-host servers to mocks without forks. Node fetch ignores HTTPS_PROXY unless NODE_USE_ENV_PROXY=1 (Node 24+) - verify per server.
- GitHub: http only on loopback; mock REST+GraphQL; raw/uploads via /raw/ and /api/uploads/. Use PAT. outputSchema doubles footprint.
- GitLab zereight: discover_tools mutates tools/list mid-session -> pin GITLAB_TOOLSETS.
- Firecrawl: setting API URL disables keyless mode -> 28 tools. Scrape has anthropic/alwaysLoad meta.
- Terraform: responses must be JSON:API.
- Cloudflare: code mode uses search over 27MB OpenAPI + execute; awkward to mock. Direct mode way too big.
- Clones (agent): /tmp/mcpr/

## Batch 2 (ops/infra/platform)
| Service | Server | Override | Tools | ~Tokens | Mode |
|---|---|---|---|---|---|
| PagerDuty | pagerduty-mcp 1.1.0 @dcfbdb8 (vendor, archived 2026-09) | PAGERDUTY_API_HOST | 63 read / 103 all | 15k / 25k | 1 |
| Datadog | official remote-only (404 tools); community winor30 1.8.0 @0fd0fd4 | DATADOG_SITE (https only, DNS+TLS) | 21 | 3k | 1 via TLS, or 2 |
| Sentry | @sentry/mcp-server 0.42.0 @c06d0b2 (vendor, FSL) | SENTRY_HOST + --insecure-http | 14 listed (76 via search/execute meta-tools) | 9.5k measured / 49k all | 1 (use --all-skills; no LLM keys) |
| Kubernetes | containers/kubernetes-mcp-server v0.1.1 @46d522b (Red Hat community) | kubeconfig server: | 22 default / 77 all | 6k / 25k | 1 (exclude pods_exec) |
| AWS | awslabs aws-api 1.6.0, cloudwatch 0.3.1, iam 1.1.2 @be74efe | AWS_ENDPOINT_URL(_SVC) | 2 + 19 + 29 | 2.5k + 13k + 7.5k | 1 (exclude suggest_aws_commands, PromQL tools; suppress deprecation warning) |
| Grafana | grafana/mcp-grafana v2.1.1 @74ddb1d (vendor) | GRAFANA_URL | 79 default / 122 all | 21k / 32k | 1 (--usage-stats=disabled) |
| Elasticsearch | elastic/mcp-server-elasticsearch 0.4.6 @9e64b84 (vendor, Rust, deprecated) | ES_URL | 5 | 1k | 1 |
| Supabase | @supabase/mcp-server-supabase 0.15.0 @ed5084d (vendor) | --api-url | 33 default / 40 all | 6-8k | 1 (mock recognises introspection SQL) |
| MongoDB | mongodb-mcp-server 3.2.2 (vendor) | MDB_MCP_API_BASE_URL (Atlas); DB tools = wire protocol | 53 | 9k | 2 (copied schemas; Atlas could be 1) |
| Okta | okta-mcp-server 1.1.7 @6fca6b0 (vendor) | OKTA_ORG_URL (forces https -> TLS) | 112 (by OKTA_SCOPES) | 30k | 1 via TLS |

## Proposed 30-service config (estimates, input schemas only)
Groups: search(59 tools/31k), code&delivery(220/56k), ops(215/61k), cloud&platform(213/57k), business(102/34k), collab(133/78k) => ~942 tools, ~315k est. Tune toolset flags down to 260k by measurement.
Mode tally: 17 real server + env override (Firecrawl, Exa, ES, GitHub, GitLab, ArgoCD, Terraform, PagerDuty, Sentry, Grafana, K8s, AWS, Supabase, Salesforce, HubSpot, Jira, Notion); 5 real server + local TLS/proxy (Brave, Tavily, Datadog, Okta, Shopify); 8 our mock MCP w/ copied schemas (Vercel, Cloudflare, Stripe, Zendesk, Slack, Linear, Google, MongoDB).

## Decision 2026-10-30: clients = pinned Claude Code + pinned Codex, every server with ALL toolsets on (no trimming)
Full-list estimate (input schemas, chars/4): search 59/31k, code&delivery 472/115k (GitLab 287/72k), ops 337/95k, cloud 260/72k, business 102/34k, collab 292/152k (Google 112/65k, Jira 98/40k)
=> ~1,520 tools, ~500k tokens. Natural surface used for Cloudflare (code mode, 4) and Sentry (14 listed + meta-tools); Vercel = community 15 (official 248 not capturable); Linear = official ~31 (community 198 would add ~40k).
Open: is 260k a target (swap services) or a floor (accept ~500k)?
Client implications: Claude Code tool search/deferral must be off so router sees full list; built-in tools (Bash etc.) can bypass MCP -> disable or sandbox; disable autoupdate/telemetry; Claude Code speaks Messages, Codex speaks Responses; Chat Completions needs a third path (verify Codex wire_api=chat status).

## Decisions round 2 (2026-10-30)
- Standardized test case; models, harnesses, router arms are run parameters. Endpoint = OpenRouter API (CC, Responses, Messages natively) -> wire format follows harness.
- Our harness = ACP client wrapper driving any ACP agent (Claude Code via ACP adapter, Codex, Pi, others). Injects the 30 MCP servers via ACP session/new mcpServers; scripts conversation; auto-permission policy; captures transcript.
- Built-in tools = harness defaults + any benchmark additions. Containment via container isolation, not disabling.
- ~150 scenarios; full or partial runs (select by id/tag/service/suite; shard).
- Containers/OCI; existing Argo deployment + Argo/Temporal benchmark flow to plug into.

## Decisions round 3 (2026-10-30)
- Harbor path approved. NO custom ACP wrapper/agent: use Harbor's existing agents (claude-code, codex, ori-pi, ori-claude, etc.) with their pins.
- Consequences: scenarios are single-instruction (no simulated-user turns); MCP servers must reach agents via Harbor task config or per-harness config files baked into env image (verify); grading via world recorder + Harbor verifier; ATIF trajectories as secondary evidence.
