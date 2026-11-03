# Authoring guide: mock services and scenarios

Read `docs/superpowers/specs/2026-10-30-tool-bench-design.md` first, sections 4–7 and 9.

## What a service needs

Each service has a crate `crates/services/<name>` (a skeleton exists for every catalog
entry) that answers every HTTP call its pinned MCP server makes. Replica services instead
answer tool calls on `/_mcp/tools/call`.

1. **Endpoint coverage.** Implement every endpoint the server's tools can call, not just the
   ones your scenarios use.
   - **Real and real-tls services:** find the endpoints in the server's source. It is installed
     in the tools image:
     - npm: `/opt/npm/node_modules/<package>`
     - Python: `/opt/py/<endpoint-id>/lib/python3.13/site-packages/`
     - Go/Rust: read the source at the pinned tag with `gh api` or `git clone` into `/tmp`
     - Browse the image with
       `docker run --rm --entrypoint sh tool-bench-tools:dev -c '<command>'`.
   - **Replica services:** implement `/_mcp/tools/call` for every tool in
     `servers/snapshots/<endpoint>.json`. Read the original server's source to see what each
     tool returns, then return the same shape. Most return JSON text in one `text` content block.
     Use `mock_core::mcp::{text, json_text, error}`.
2. **Realistic responses.**
   - Status codes, error bodies, pagination, and envelope shapes must match the real API.
   - Endpoints your scenarios don't care about return realistic data derived from the dataset
     (empty lists, the real "not found" error), never a generic stub.
   - The server's tool-output formatting depends on these shapes, so check it against the
     server's own parsing code.
3. **Determinism.**
   - No wall clock and no randomness.
   - Times come from `world.clock` (`now_rfc3339()`); it ticks once per mutating request.
   - New IDs come from the dataset (for example, max + 1) or from `ServiceState.ids`.
   - Lists iterate in dataset order (`Collection` is insertion-ordered) or sort by an explicit key.
4. **State.**
   - Read and write `world.service_mut(NAME).store`, one `Collection` per resource type.
   - Key documents so the grader can address them with a JSON pointer, for example
     `"<owner>/<repo>#<number>"`.
   - The final state is graded through `/_bench/state`: `{"services": {"<name>": {"<collection>": {"<key>": doc}}}}`.
   - Datasets are `scenarios/<id>/dataset/<service>/<collection>.json` (JSON arrays; `_key`,
     falling back to `id`, is the key).
5. **Tests.** `cargo test -p mock-<name>` must cover each endpoint family: happy path, the
   real 404/422 shapes, pagination, and mutations reflected in state. Follow the pattern in
   `crates/services/github/tests/github.rs`.

Use `crates/services/github` and `crates/services/okta` as structure references.

### Boundaries (parallel work)

- Edit only:
  - your own crates
  - your own `scenarios/<group>/<service>/` directories
  - your services' entries in `catalog/services.toml`, and only if a server truly needs a
    new TLS hostname or env setting
- Do **not** edit `mock-core`, `world`, `mcp-tools`, `catalog` (Rust), `scenario`, `taskgen`,
  or other services. If you need a shared helper, keep a private copy in your crate and
  mention it in your report.
- If you change catalog `env` or `args`, the tools image must be rebuilt for the change to
  apply. Build your own tag and pass it to the oracle:

  ```bash
  docker build -f images/tools/Dockerfile -t tool-bench-tools:$(git rev-parse --abbrev-ref HEAD | tr / -) .
  TOOLS_IMAGE=tool-bench-tools:<that-tag> deploy/oracle.sh ...
  ```

  Avoid this unless it is needed.

## Scenarios

There are 4 per service, at `scenarios/<group>/<service>/<slug>/`:

| File | Content |
|---|---|
| `scenario.toml` | `[scenario]` (id = path, title, now, seed, services, tags, timeout_sec), `[[expect.*]]`, `[[solution.call]]`, optional `[solution] answer` |
| `instruction.md` | What a real user would ask. No tool names, API paths, or hints about which service has the data, unless a real user would name it ("in our GitHub Enterprise"). If you expect an answer, ask for it to be written to `/app/answer.md`. |
| `dataset/<service>/*.json` | The world this scenario starts from. Curated, realistic, and small enough to read. Include distractors: similar names, closed vs. open items, other projects. |

Mix per service:

- **1 lookup:** the answer is checked with `[expect.answer]`, using `contains` or `regex`.
- **2 mutations:** checked with `[[expect.state]]` plus `[[expect.required_calls]]`, with
  `[[expect.forbidden_calls]]` guarding destructive mistakes.
- **1 multi-step or disambiguation task:** for example, finding the right record among
  look-alikes before changing it.

Tag each scenario with:

- `lookup` or `write`
- `single-service`
- the service name
- `disambiguation` where it applies

Some records get keys that depend on what the agent does, such as a new comment's position
or an id that other writes shift. For those, use `any` with `key_prefix` instead of pointing
at a guessed key:

```toml
[[expect.state]]
pointer = "/services/zendesk/comments"
key_prefix = "4217/"
any = { public = true, body = "RF-88213" }   # strings match by substring
```

Expectations must be **precise**: a correct agent can't fail them, and an incorrect one can't
pass them. Check final state over individual calls wherever possible, since agents may reach
the same result in different ways. Use `required_calls` only for what must happen, such as the
create itself, without over-specifying query parameters.

`[[solution.call]]` entries use real tool names and arguments from
`servers/snapshots/<endpoint>.json`. The oracle replays them through the actual MCP server.

## Verification (all must pass before you report)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p scenario --bin taskgen          # validates every scenario, regenerates tasks/
deploy/oracle.sh <every scenario id you added>  # oracle reward 1 and nop reward 0 for each
```

Commit `tasks/` with your scenarios; it is generated, so never edit it by hand. Commit with
clear messages on your branch. Your final report should list:

- the endpoints implemented per service
- the scenarios added
- the `oracle.sh` output
- anything you could not make work
