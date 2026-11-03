# tool-bench

A deterministic benchmark for tool calling with very large tool sets: 30 real services, about
1,500 MCP tools, and more than 600k tokens of tool definitions. Agents run through Harbor on the
OpenRouter benchmark flow and call real (pinned) MCP servers. Those servers talk to mock APIs
written in Rust, which answer like the real services from curated per-scenario datasets.

Design: `docs/superpowers/specs/2026-10-30-tool-bench-design.md`. Operating it: `docs/runbook.md`.

## Status

- **30 services, 32 MCP endpoints, 1,595 tools, ~727k o200k tokens** of tool definitions (`docs/research/tool-payload.md`), captured offline and hash-identical across runs.
- **150 scenarios** (120 single-service, 30 cross-service), each a generated Harbor task in `tasks/`. Every one scores reward 1 with Harbor's oracle agent (replaying the solution over MCP) and 0 with the nop agent.
- **Determinism:** logical clock, seeded IDs, recorded journal; `world-replay` re-issues a trial's journal against a fresh world and must match byte for byte.
- **Model smoke run:** 3 tasks × 2 modes with Claude Code + Claude Haiku 5.5 via Ori/OpenRouter, all reward 1.0 (`docs/research/smoke-2026-10-31.md`, `deploy/smoke-ori.sh`). Full payload ≈ 820k Claude tokens per turn.
- **Deferred-tools arms:** universal vs native ToolSearch vs all-tools-upfront, 324 runs over Claude Haiku 5.5 and Kimi K2.7 Code (`docs/research/arms-2026-10-31.md`, `deploy/smoke-arms.sh`).
- **Argo:** images are published on merge to `main` (`docs/runbook.md`); the Harbor runner registers this directory's `tasks/`.
- **Provenance and licences** of vendored upstream data: `docs/provenance.md`.

## Layout

| Path | What |
|---|---|
| `catalog/services.toml` | Source of truth for the 30 services and 32 MCP endpoints: pins, launch commands, env, ports, TLS hostnames |
| `crates/mock-core` | Shared mock library: logical clock, deterministic IDs, document store, datasets, request journal, admin API |
| `crates/services/*` | One mock API per service |
| `crates/world` | Binary hosting every mock on its port, plus HTTPS on 443 (local CA) for fixed-hostname clients |
| `crates/mcp-tools` | Stdio↔streamable-HTTP bridge, replica backend, MCP clients, `toolmeter` (capture, report, import) |
| `crates/catalog` | Catalog parser and CLI, plus `tools-supervisor` (the tools container entrypoint) |
| `servers/` | npm and Python lockfiles, per-server config, capture scripts, tool-list snapshots |
| `images/` | `world` and `tools` Dockerfiles |
| `deploy/` | Local compose stack and the offline capture script |
| `scenarios/` | Scenario sources (`scenario.toml`, `instruction.md`, `dataset/`) |

## Common tasks

```bash
cargo test --workspace                                    # all tests
cargo run -p catalog --bin catalog -- validate            # check the catalog

docker build -f images/world/Dockerfile -t tool-bench-world:dev .
docker build -f images/tools/Dockerfile -t tool-bench-tools:dev .

docker compose -f deploy/compose.dev.yaml up              # world + tools; MCP on localhost:7101-7132
./deploy/capture-stack.sh                                 # offline capture of every endpoint -> servers/snapshots
cargo run -p mcp-tools --bin toolmeter -- report          # table of tools, bytes and tokens per endpoint
```

The world on its own:

```bash
SCENARIO_DIR=scenarios/example/github/basic cargo run -p world
curl localhost:8101/api/v3/repos/octo-org/app/issues
curl localhost:9000/_bench/journal
```
