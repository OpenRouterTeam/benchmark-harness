# Runbook: building, running and comparing tool-bench

## 1. Images

Tasks reference `us-docker.pkg.dev/openrouter-ci/harbor/tool-bench-{world,tools}:<tag>`, where each
tag is a hash of the files that image is built from (`crates/scenario/src/images.rs`). After changing
scenarios, crates, servers, the catalog or an image Dockerfile, regenerate and commit `tasks/`:

```bash
cargo run -p scenario --bin taskgen
```

CI (`.github/workflows/tool-bench.yml`) runs `taskgen --check`, so a stale `tasks/` fails the PR.
On merge to `main`, `.github/workflows/tool-bench-images.yml` builds and pushes any tag that isn't
published yet, so the tasks on `main` always point at images built from the same sources.
`taskgen --print-images` prints the two references. The trial pulls them with its Workload
Identity; compose `image:` lines are not rewritten by the trial's mirror logic.

For local runs, pass local tags: `--world-image tool-bench-world:dev --tools-image tool-bench-tools:dev`
(`deploy/oracle.sh` does this).

## 2. Register the benchmark

`tool-bench` is registered in the Harbor runner (`services/gcp-harbor-trial/src/gcp_harbor_trial/benchmarks.py`
in openrouter-web) as this repository's `main`, tasks path `tool-bench/tasks`.

## 3. Run

Submit through the Argo `benchmark-request` template, like any Harbor benchmark. The fields
that matter here:

| Field | Use |
|---|---|
| `benchmarkId` | `tool-bench` |
| `agent.name` / `agent.modelName` | `claude-code`, `codex`, or `acp`; any OpenRouter slug |
| `agent.kwargs.version` | harness pin, e.g. Claude Code `2.1.235` |
| `attempts` | repeats per task (≥ 3 recommended for paired comparisons) |
| `tasks.include` / `tasks.exclude` | globs over `<group>__<service>__<slug>`, e.g. `["ops__*"]`, `["*__multi__*"]` |
| `experimentIds` | router arm (sent as `x-openrouter-experiment-ids`) |
| `requestOverlay.extra_body` | router tool-handling fields, e.g. `{"deferred_tools": {...}}` |

Run each arm as its own run, with everything else identical.

## 4. Read results

- Per-trial metrics in `reward.json`:
  - `reward`
  - `outcome_code`: 0 pass, 1 fail, 2 context overflow
  - `state_ok`, `required_calls_ok`, `forbidden_calls_ok`, `answer_ok`
  - `api_calls`, `mcp_bypass`
- The verifier also keeps `journal.json` (every API call) and `state.json` (final world
  state) next to `reward.json`.
- The world's admin port (`world:9000`) answers only `/_bench/health` and `/_bench/ca.pem` without
  the per-task token in `tests/admin-token`, which Harbor uploads only when verification starts. The
  agent shares the network with the world but cannot read the state or call the grader.
- To compare arms, download both runs' artifacts
  (the run's artifact bucket) and run:

  ```bash
  cargo run -p report --bin bench-report -- runs/arm-a runs/arm-b          # markdown
  cargo run -p report --bin bench-report -- runs/arm-a runs/arm-b --json
  ```

## 5. Verify determinism of any trial

```bash
cargo run -p world --bin world-replay -- scenarios/<scenario-id> <trial>/verifier/journal.json
```

This replays every API call against a fresh world and fails on any differing response. The
tool lists themselves are covered by `deploy/capture-stack.sh`: run it twice and compare the
`sha256` of every snapshot.

## 6. Local checks before publishing

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cargo run -p scenario --bin taskgen -- --check       # tasks/ matches scenarios/
deploy/oracle.sh --all                               # oracle = 1 and nop = 0 for every task
```
