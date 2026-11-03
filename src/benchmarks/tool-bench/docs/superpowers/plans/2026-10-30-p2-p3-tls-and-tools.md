# P2 + P3: TLS Redirect and Tools Image — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let every real MCP server reach the mock world unmodified (including the 5 that only speak HTTPS to a fixed host), and build the `tools` container: all 32 MCP endpoints (24 real server processes, 8 replicas) served over streamable HTTP, with every tool list captured, hashed, and token-measured.

**Architecture:**
- **P2:** The world generates a local certificate authority (CA) at startup and one leaf certificate covering every real hostname the services declare. It serves HTTPS on 443 and dispatches by `Host` to the matching service app. The CA certificate is published at `GET /_bench/ca.pem`. The compose file gives the world container network aliases for those hostnames, so `api.search.brave.com` resolves to the world. The tools container fetches the CA at startup and trusts it through the usual env vars (`NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`).
- **P3:** A single catalog file (`catalog/services.toml`) is the source of truth for each service: group, mode, world port, hostnames, MCP port, the pinned server and its launch command/args/env.
  - **`mcp-bridge` (Rust):** exposes every stdio server as streamable HTTP. One child process per MCP session; env comes from the catalog.
  - **`replica-mcp` (Rust):** serves captured tool lists verbatim for replica services.
  - **`toolmeter` (Rust):** captures `tools/list` from any endpoint and writes `servers/snapshots/<name>.json`, with SHA-256 hashes and token counts (o200k offline; Anthropic `count_tokens` only when a key is set).
  - **`images/tools/Dockerfile`:** installs every pinned server.

**Tech stack:** existing workspace, plus axum-server 0.8 (`tls-rustls`), rcgen 0.14, sha2, tiktoken-rs 0.12, Docker.

**Spec:** `docs/superpowers/specs/2026-10-30-tool-bench-design.md` (§3.1, §3.3, §4, §5.3, §9, §12 D1).

## Global constraints
- Tool definitions are never edited. A rejected schema is a recorded result (spec §4).
- Every server is pinned to an exact version; tool-list hashes are recorded (spec §9).
- Runtime-mutating or non-deterministic server options are pinned off (spec §4 rules).
- No secrets in the repo. Credentials are fixed fake strings the mocks accept.
- Rust gates: `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace` pass at every commit.

## Tasks

### P2.1 Local CA and leaf certificate (`crates/world/src/tls.rs`)
- `LocalCa::generate(hosts: &[&str]) -> anyhow::Result<LocalCa>` with `ca_pem: String`, `leaf_cert_pem: String`, `leaf_key_pem: String`.
- Tests: the leaf verifies against the CA for every requested host, and for no other host (rustls `WebPkiServerVerifier`). The CA is marked as a CA; the leaf's SANs equal `hosts`.

### P2.2 Host dispatch and HTTPS listener
- `host_router(world, services) -> Router`: looks at `Host` (port stripped, lowercased) and forwards to that service's `service_app`. Unknown host → 421 Misdirected Request with a JSON body. Sets `x-forwarded-proto: https`, so absolute URLs come out `https://`.
- `spawn` gains the TLS listener (port 443 fixed, ephemeral in tests) and `Running.tls: SocketAddr`. Admin gains `GET /_bench/ca.pem`.
- Tests (reqwest with only the generated CA as root, `.resolve(host, addr)`): `https://api.github.com/api/v3/user` returns the viewer. The journal records the request under `github`. An unknown host returns 421. `/_bench/ca.pem` equals `ca_pem`.

### P3.1 Catalog (`catalog/services.toml`) + parser (`crates/catalog`)
- Parser for `[[service]]` entries: `name, group, mode (real|real-tls|replica), world_port, hosts[], mcp_port`, and `[[service.server]]` entries (`id, package, version, runtime (npm|pypi|go|cargo|binary), command, args[], env{}`).
- Validation tests: names, world ports, and MCP ports are unique; real modes have at least one server; replica modes have no server; every `{world}` placeholder in env resolves.
- Data for all 30 services, taken from spec §4 and the research notes, with every toolset enabled.

### P3.2 `mcp-bridge` (`crates/mcp-bridge`)
- Streamable-HTTP front (`POST /mcp`; JSON responses; `Mcp-Session-Id`; `DELETE` ends the session) over one stdio child per session. The child is started from the catalog command/args/env. Forwards JSON-RPC by `id`. Server→client requests get a JSON-RPC error `-32601` (no client capabilities offered). Notifications from the child are dropped. Session IDs come from a counter.
- Tests use a scripted fake stdio server (a test binary): initialize → tools/list round-trip; two sessions get separate children; `DELETE` kills the child; the child's env is exactly the configured env plus `PATH`/`HOME`.

### P3.3 `toolmeter` (`crates/toolmeter`)
- `toolmeter capture --stdio <cmd…> | --http <url> --name <n>`: initialize, paginate `tools/list` fully, write `servers/snapshots/<n>.json` = `{name, server, version, tools:[…verbatim…], sha256, counts:{tools, bytes, o200k}}`.
- `toolmeter report`: a markdown table of every snapshot plus totals, flagging any schema with `$schema` draft-04/07 or a missing `type: object` (lint only).
- Tests: o200k count of a known string; sha256 is stable under re-capture; pagination via `nextCursor` is followed.

### P3.4 Capture real servers and replica schemas
- Install each pinned server in a throwaway Linux container (`images/tools/Dockerfile` stages) and run `toolmeter capture` against it. Commit the snapshots.
- Replica sources:
  - **Slack:** the official remote tool definitions loaded in this environment; caller IDs replaced with fixed fake IDs.
  - **Stripe:** Kiln-AI `stripe_world` `real_tool_schemas.json`.
  - **Zendesk, MongoDB, Vercel, Google Workspace:** run the community/vendor server once over stdio.
  - **Cloudflare:** the `cloudflare/mcp` tool definitions from source.
  - **Linear:** the official schemas need Linear auth (D5). Interim: the community `@tacticlaunch/mcp-linear` 1.4.5 schemas, recorded as a deviation.

### P3.5 `replica-mcp` (`crates/replica-mcp`)
- Streamable-HTTP MCP server per replica service: `initialize` (server name/version from the snapshot), `tools/list` (the snapshot's tools, verbatim, one page), `tools/call` → a per-tool handler registry. Until P4 adds handlers, an unregistered tool returns an MCP tool error (`isError: true`, text `"<tool> is not available in this scenario"`).
- Tests: `tools/list` is byte-equal to the snapshot tools; an unknown method → -32601; the error path works.

### P3.6 Tools image + compose
- `images/tools/Dockerfile`: multi-stage, pinned base from the mirror, every server installed at its catalog version, bridge, replica-mcp, an entrypoint that fetches the CA, sets the trust env vars, and starts one bridge/replica per catalog entry on its MCP port.
- `images/world/Dockerfile`: the world binary plus the scenarios.
- `deploy/compose.dev.yaml`: world (with host aliases) + tools, for local runs.
- Verification: `docker compose up`, then `toolmeter report` across all 32 endpoints. Record total tools and tokens in `docs/research/tool-payload.md`.
