# Tool-Bench: Large-Tool-Set Agentic Benchmark — Design

Date: 2026-10-30
Status: Approved 2026-10-30
Research inputs: `docs/research/mcp-servers.md`, internal benchmark-runner notes (not published)

## 1. Goal

A standardized, fully deterministic benchmark that measures how well a **model + harness + OpenRouter router arm** handles an extremely large tool set. The headline use is A/B testing router tool-handling changes: show that a new arm completes tasks when the full tool payload exceeds the context window of the model under test.

- 30 real services, each exposed to the agent through its **real (pinned) MCP server** where one can be pointed at a custom URL, otherwise through **our MCP server serving the real tool schemas verbatim**.
- Every server runs with **all toolsets enabled**. No trimming. Expected payload ≈ 1,500 tools, ≈ 500k tokens of tool definitions (estimate; measured per run). **260k tokens is a floor, not a target.**
- Behind every tool is a **mock API written in Rust** that responds like the real service, from curated per-scenario datasets.
- ~150 scenarios. Any subset can be run.
- Runs on the existing OpenRouter **Argo → Harbor** benchmark flow.

### Non-goals

- Live data or live vendor APIs. Nothing leaves the pod except model traffic to OpenRouter.
- A custom agent harness. Harbor's existing agents run the harnesses.
- Trimming tool sets to fit a model. Overflow is a measured outcome.
- Multi-turn simulated users. Each scenario is a single instruction.

## 2. Terms

| Term | Meaning |
|---|---|
| Scenario | One test case: instruction, dataset, expected outcome. Becomes one Harbor task. |
| Dataset | The curated world state a scenario starts from (e.g. one GitHub org, one PagerDuty account). |
| World | The container running all 30 mock APIs, the recorder, and the TLS redirect proxy. |
| Tools | The container running all 30 MCP servers. |
| Main | Harbor's agent container, where the harness (Claude Code, Codex, ACP agent, …) runs. |
| Arm | One router configuration under test: model slug, provider pinning, `experimentIds`, request overlay. |
| Server mode | How a service's tools reach the mock: **real** (vendor/community server + URL override), **real-TLS** (same, via local CA + redirect proxy), **replica** (our MCP server with copied real schemas). |

## 3. Architecture

### 3.1 Trial topology (one Harbor trial = one scenario attempt)

```
 ┌──────────── Harbor trial pod (Docker-in-Docker, compose) ────────────┐
 │                                                                       │
 │  main (agent)              tools                     world            │
 │  ───────────               ─────                     ─────            │
 │  harness CLI  ──HTTP MCP──▶ 30 MCP servers ──HTTP──▶ 30 mock APIs     │
 │  (claude-code,             (22 real pinned,          recorder         │
 │   codex, acp, …)            8 replica, stdio         TLS redirect     │
 │      │                      servers behind a          proxy + local CA│
 │      │                      pinned stdio→HTTP         scenario dataset│
 │      │                      bridge)                                   │
 │      ▼                                                                │
 │  Harbor session proxy ──▶ OpenRouter (Chat Completions / Responses /  │
 │  (session id, experiment      Messages, chosen by the harness)       │
 │   ids, request overlay)                                               │
 └───────────────────────────────────────────────────────────────────────┘
```

- `main` sees MCP servers only as `streamable-http` URLs declared in `task.toml` `[[environment.mcp_servers]]` (e.g. `http://tools:7001/mcp`). Harbor's `claude-code`, `codex`, and `acp` agents register them automatically.
- `tools` holds every server's env (`GITHUB_HOST=http://world:8101`, …). None of it is visible in `main`.
- `world` holds the dataset. It is never mounted in `main`.
- Agent network policy: `allowlist` = OpenRouter hosts (already added by the Harbor runner) + `tools`. Direct calls from `main` to `world` are possible only if the allowlist does not cover sibling services; the recorder tags any request that did not come from `tools` as **MCP bypass** (section 7).

### 3.2 Repository layout

```
tool-bench/
  Cargo.toml                    # Rust workspace
  crates/
    mock-core/                  # shared mock library (P1)
    world/                      # binary: hosts all mock services
    services/<name>/            # one crate per mock API (30)
    replica-mcp/                # binary: serves copied schemas for replica-mode services
    grader/                     # binary: reads recorder state, writes reward.json
    taskgen/                    # binary: scenarios/ -> tasks/ (Harbor task dirs)
    toolmeter/                  # binary: tools/list capture, hash, token counts
  servers/                      # pinned real MCP servers: lockfiles, launch config, bridge config
  scenarios/<group>/<id>/       # scenario source: scenario.toml, instruction.md, dataset/
  tasks/                        # GENERATED Harbor task dirs (committed, reviewed in diffs)
  images/world/, images/tools/  # Dockerfiles
  docs/
```

### 3.3 Images

- `world`: Rust static binary + scenario datasets for all scenarios (dataset chosen at startup by `SCENARIO_ID`). Built in this repo's CI, pushed to `$REGISTRY/tool-bench-world:<sha>`.
- `tools`: all 22 real servers installed offline at pinned versions (npm/PyPI/Go/Rust lockfiles), the replica-mcp binary, the stdio→HTTP bridge. Pushed as `tool-bench-tools:<sha>`.
- `main`: Harbor's default task image plus nothing benchmark-specific. The harness is installed by the Harbor agent at the pinned version.
- Third-party base images come through a registry mirror.

Each generated task's `docker-compose.yaml` references `world` and `tools` by immutable digest.

## 4. Service catalog

All 30 services, all toolsets on. Versions are pins; counts and tokens are from `docs/research/mcp-servers.md` and are re-measured by `toolmeter` (P3).

| Group | Service | Mode | Server (pin) | URL override |
|---|---|---|---|---|
| Search & web | Firecrawl | real | `firecrawl-mcp` 3.36.0 | `FIRECRAWL_API_URL` |
| | Brave Search | real-TLS | `brave-search-mcp-server` 2.1.4 | redirect proxy |
| | Exa | real-TLS | `exa-mcp-server` 3.4.2 | TLS alias `api.exa.ai` (the bundle ignores `EXA_API_BASE_URL`) |
| | Tavily | real-TLS | `tavily-mcp` 0.2.22 (0.2.23 was never published) | TLS alias `api.tavily.com` |
| | Elasticsearch | real | `mcp-server-elasticsearch` 0.4.6 | `ES_URL` |
| Code & delivery | GitHub | real-TLS | `github-mcp-server` 2.0.3 | `GITHUB_HOST=https://github.octo-corp.com` (plain HTTP is loopback-only) |
| | GitLab | real | `@zereight/mcp-gitlab` 2.3.16 | `GITLAB_API_URL`, `GITLAB_GRAPHQL_URL` |
| | ArgoCD | real | `argocd-mcp` 0.9.0 | `ARGOCD_BASE_URL` |
| | HCP Terraform | real-TLS | `terraform-mcp-server` 1.4.1 | `TFE_ADDRESS` + TLS alias `registry.terraform.io` |
| | Vercel | replica | community `mcp-vercel` schemas | n/a |
| Ops & health | PagerDuty | real | `pagerduty-mcp` 1.1.0 | `PAGERDUTY_API_HOST` |
| | Datadog | real-TLS | `mcp-server-datadog` 1.8.0 | `DATADOG_SITE` + local CA |
| | Sentry | real | `@sentry/mcp-server` 0.42.0 | `SENTRY_HOST` + `--insecure-http` |
| | Grafana | real | `mcp-grafana` 2.1.1 | `GRAFANA_URL` |
| | Kubernetes | real | `kubernetes-mcp-server` 0.1.1 | kubeconfig `server:` |
| Cloud & platform | AWS | real | awslabs `aws-api` 1.6.0, `cloudwatch` 0.3.1, `iam` 1.1.2 | `AWS_ENDPOINT_URL*` + TLS alias `servicereference.us-east-1.amazonaws.com` |
| | Cloudflare | replica | `cloudflare/mcp` code-mode schemas | n/a |
| | Supabase | real | `@supabase/mcp-server-supabase` 0.15.0 | `--api-url` |
| | MongoDB | replica | `mongodb-mcp-server` 3.2.2 schemas | n/a (wire protocol) |
| | Okta | real-TLS | `okta-mcp-server` 1.1.7 | `OKTA_ORG_URL` + local CA |
| Business ops | Stripe | replica | captured remote `mcp.stripe.com` schemas | n/a |
| | Salesforce | real | `@tsmztech/mcp-server-salesforce` 0.0.8 | `SALESFORCE_INSTANCE_URL` |
| | HubSpot | real | `@hubspot/mcp-server` 0.4.0 | `BASE_URL_OVERRIDE` |
| | Zendesk | replica | `reminia/zendesk-mcp-server` schemas | n/a |
| | Shopify | real-TLS | `shopify-mcp` 1.0.8 | `MYSHOPIFY_DOMAIN` + local CA |
| Collaboration | Slack | replica | captured remote `mcp.slack.com` schemas | n/a |
| | Linear | replica | interim: `@tacticlaunch/mcp-linear` 1.4.5 schemas (198 tools); official `mcp.linear.app` capture needs Linear auth (D5) | n/a |
| | Jira | real | `mcp-atlassian` 0.23.1 (all 98 tools incl. Confluence) | `JIRA_URL`, `CONFLUENCE_URL` |
| | Notion | real | `@notionhq/notion-mcp-server` 2.6.1 | `BASE_URL` |
| | Google Workspace | replica | `google_workspace_mcp` 2.2.1 schemas (all 112) | n/a |

The authoritative, machine-readable version of this table is `catalog/services.toml`; measured tool counts and token sizes are in `docs/research/tool-payload.md`.

Rules:
- Server settings that change the tool list at runtime are pinned off (GitLab `discover_tools` → fixed `GITLAB_TOOLSETS=all`; Sentry `--all-skills`).
- Settings that make servers non-deterministic are disabled: Sentry LLM keys unset, AWS `suggest_aws_commands` endpoint pointed at `world`, Grafana `--usage-stats=disabled`, Salesforce `--no-telemetry`, Notion/Slack descriptions frozen at capture.
- Tool definitions are never edited. If a schema is rejected by a provider, that is a recorded result, not something we patch.
- Every server runs with `PYTHONHASHSEED=0`. A server whose `tools/list` order is itself random (Elasticsearch's rmcp server) is marked `sort_tools`, and the bridge returns its list sorted by name. Order is the only thing changed.
- Capture is verified by running the offline stack twice: every endpoint must hash identically.
- Known upstream nondeterminism in tool *results* (not in the world): `mcp-server-elasticsearch` re-serializes index mappings through a hash map, so property order in `get_mappings` output varies between runs. World responses themselves stay byte-identical (`world-replay`).

## 5. World: the mock APIs

### 5.1 Behaviour contract

1. **Same input, same output.** A world started with scenario S answers request sequence R with byte-identical responses every time.
2. **Looks real.** Status codes, headers (pagination `Link`, rate-limit headers, request IDs), error bodies, and JSON shapes follow the real API.
3. **No unimplemented endpoints.** Every endpoint any of the ~1,500 tools can call returns a realistic response. Endpoints a scenario cares about read and mutate the dataset; others return deterministic defaults derived from the dataset (empty lists, plausible objects, or the service's real 404).
4. **Everything is recorded.** Every request and response is journaled in order.

### 5.2 mock-core (built in P1)

| Module | Responsibility |
|---|---|
| `clock` | Logical clock. Starts at the scenario's `now`. Advances a fixed step per mutating request. No wall-clock reads anywhere. |
| `ids` | Deterministic ID generators per service and kind: counters, prefixed IDs (`cus_…`), UUIDv5/v8 from (scenario, service, kind, counter), GitHub-style node IDs. |
| `store` | In-memory document store per service: named collections of JSON documents with ordered iteration, loaded from the dataset, mutable, snapshot-able. |
| `dataset` | Loads `scenarios/<id>/dataset/<service>/*.json` (or the world image's copy) and the scenario's `now`/seed. |
| `journal` | Append-only record: seq, logical time, service, source peer, method, path, query, request body, status, response body. |
| `admin` | `/_bench/health`, `/_bench/journal`, `/_bench/state`, `/_bench/reset` (reload dataset). Bound only on the admin port. |
| `http` | Service-style helpers: JSON errors per service, pagination (Link header, cursor, offset), JSON:API envelopes, GraphQL request envelope parsing, consistent request-ID headers. |
| `service` | `MockService` trait: name, port, hostnames it answers for (TLS redirect), router builder over shared state. |
| `canonical` | Stable JSON output: sorted keys where the real API does not define order, fixed float formatting. |

### 5.3 Routing

- Each service listens on a fixed port (`8101` GitHub, …; table generated from the catalog).
- The TLS redirect proxy terminates TLS for real hostnames (`api.search.brave.com`, `*.okta.com`, …) using the local CA and forwards by `Host` to the service port.
- One world process serves exactly one scenario (selected at start). `/_bench/reset` reloads it for local iteration.

### 5.4 Protocols

REST (most), GraphQL (GitHub partial, Shopify, Linear, GitLab partial), JSON:API (HCP Terraform), Query/XML (AWS EC2, IAM), REST-XML (S3), JSON 1.1 (CloudWatch Logs), Kubernetes API discovery. mock-core provides envelopes and helpers; protocol engines beyond REST are added in P4 inside the services that need them.

## 6. Scenarios and tasks

### 6.1 Scenario source format

```
scenarios/<group>/<service-or-multi>/<id>/
  scenario.toml     # id, title, tags, services, now, seed, timeouts, grading
  instruction.md    # the single user instruction
  dataset/<service>/*.json
```

`scenario.toml` grading section declares:
- `expect.state`: assertions over the final world state (JSONPath-style selectors with expected values).
- `expect.calls.required`: calls that must appear (service, method, path pattern, optional body matcher).
- `expect.calls.forbidden`: calls that must not appear (e.g. destructive operations on the wrong resource).
- Optional `expect.answer`: exact or regex checks on the agent's final message (only where the task is a question).

### 6.2 Generated Harbor task

`taskgen` writes `tasks/<group>/<service-or-multi>/<id>/` with `task.toml` (resources, timeouts, agent allowlist, 30 `[[environment.mcp_servers]]` entries), `instruction.md`, `environment/docker-compose.yaml` (world + tools by digest, `SCENARIO_ID`), `tests/test.sh` (calls the grader). Task names encode group/service/id so Harbor `tasks.include` globs select subsets (`code/github/*`, `*/multi/*`).

Coverage target: 150 scenarios, 5 per service (≥1 read-only lookup, ≥1 multi-step mutation, ≥1 needing disambiguation between similar tools across services), and about a third spanning 2–4 services.

## 7. Grading

`[[verifier.collect]]` runs `curl world:9000/_bench/state` and `/_bench/journal` into artifacts; the verifier runs `grader`, which writes `/logs/verifier/reward.json`:

| Metric | Meaning |
|---|---|
| `reward` | 1.0 pass / 0.0 otherwise (primary) |
| `state_ok`, `required_calls_ok`, `forbidden_calls_ok`, `answer_ok` | component checks |
| `tool_calls`, `distinct_tools`, `wrong_service_calls` | behaviour counts |
| `mcp_bypass` | count of world requests not originating from `tools` |
| `outcome` | `pass`, `fail`, `context_overflow`, `harness_error`, `mcp_bypass` (encoded numerically; mapping in grader docs) |

`context_overflow` is set when the harness trajectory or proxy logs show the provider rejected the request for length, or when no model turn completed and the trajectory contains a context-length error. Grading never uses an LLM judge.

## 8. Runs and arms

A run = Harbor run spec through `benchmark-request`: model slug(s), provider pinning, `experimentIds`, `requestOverlay`, harness agent + version, `tasks.include/exclude`, `attempts`.

- Harnesses: Harbor `claude-code`, `codex`, `acp` (any ACP agent). Pi requires a gap fix (section 10).
- Harness tool-handling settings are part of the run config and recorded (e.g. Claude Code tool deferral on/off).
- Every trial records: harness + version, wire format, tool-list hash per server, measured tool payload tokens, arm config.

## 9. Determinism

- World: logical clock, deterministic IDs, ordered iteration, canonical JSON, no randomness, no network egress.
- Tools: pinned server versions installed offline; tool-list hash per server recorded each trial; a changed hash marks the trial non-comparable.
- Check: `world` replay test in CI — replay a recorded journal against a fresh world and require byte-identical responses.
- Only model sampling may vary between attempts.

## 10. Changes needed outside this repo (the benchmark runner)

1. Register the benchmark in the Harbor runner's benchmark registry (`git_url`, `ref`, `tasks_path`).
2. Extend `RequestOverlay` + `apply_request_overlay` in `proxy.py` to carry router tool-handling fields (e.g. `deferred_tools` / extra body), or confirm `experimentIds` is enough for the arms planned.
3. Paired-arm report: per-scenario comparison between two runs with confidence intervals (new; aggregates exist today).
4. Pi: add MCP registration to `ori.py`, or run Pi through Harbor's `acp` agent with a Pi ACP adapter.

## 11. Phases

| Phase | Deliverable |
|---|---|
| 0 | Harbor verification — done |
| P0 | This spec + implementation plan for P1 — done |
| P1 | `mock-core` + `world` skeleton + one example service, fully tested — done |
| P2 | Local CA + TLS redirect proxy — done |
| P3 | Tools image: pinned servers, bridge, replica-mcp, toolmeter — done |
| Slice | GitHub + Firecrawl + PagerDuty, 5 scenarios, through Argo `benchmark-run` with `claude-code` and `codex` — local Harbor slice done (oracle/nop) plus a model smoke run (docs/research/smoke-2026-10-31.md). Out of scope by decision (2026-10-31): creating the OpenRouterTeam/tool-bench repo, publishing images, and the Argo run are left to the owner (docs/runbook.md) |
| P4 | Mock APIs for all 30 services — done |
| P5 | 150 scenarios + grader + taskgen — done (120 single-service + 30 cross-service) |
| P6 | openrouter-web changes (section 10) — draft PR in the benchmark runner |
| P7 | Validation: replay determinism, baseline runs, docs — determinism and oracle/nop validation done; model baseline runs on Argo left to the owner with publishing (decision 2026-10-31) |

## 12. Decisions

| # | Decision | Status |
|---|---|---|
| D1 | Where MCP servers run | **Decided (user, 2026-10-30):** `tools` compose service, all over streamable HTTP (stdio servers behind a pinned bridge) |
| D2 | Pi support path | Open, default: deferred to P6; Claude Code + Codex + ACP first |
| D3 | Does the agent allowlist gate `main` → `world`? | Open: verify in the vertical slice; bypass is recorded either way |
| D4 | Router tool-handling field transport | Open, default: extend `RequestOverlay` (P6) |
| D5 | Linear official schema capture | Open: needs Linear MCP authorized; fall back to published tool list |
