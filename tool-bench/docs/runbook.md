# Runbook: building, running and comparing tool-bench

## 1. Build and publish images

```bash
TAG=$(git rev-parse --short HEAD)
REG=$REGISTRY  # any registry the trial's Docker daemon can pull from
docker buildx build --platform linux/amd64 -f images/world/Dockerfile -t $REG/tool-bench-world:$TAG --push .
docker buildx build --platform linux/amd64 -f images/tools/Dockerfile -t $REG/tool-bench-tools:$TAG --push .
```

Regenerate the tasks against those references and commit `tasks/`:

```bash
cargo run -p scenario --bin taskgen -- --world-image $REG/tool-bench-world:$TAG --tools-image $REG/tool-bench-tools:$TAG
```

Compose `image:` lines are not rewritten by the trial's mirror logic. The images must be in
a registry the trial's Docker daemon can pull from.

## 2. Register the benchmark

Register `tool-bench` in the Harbor runner's benchmark registry with `tasks_path = tasks/`.

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
