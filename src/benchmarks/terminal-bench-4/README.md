# Terminal-Bench 4.0

[Terminal-Bench 4.0](https://www.tbench.ai/news/terminal-bench-4-0) is the current major release of the continuous Terminal-Bench line that began with 3.0. It is a different task set from Terminal-Bench 2.1 (`terminal_bench` in this catalog), so scores are not comparable across the two benchmarks. This benchmark is registered separately as `terminal_bench_4`.

## Source & license

- Repository: <https://github.com/harbor-framework/terminal-bench>, pinned to the `v4.0.0` tag, commit `452bf305c6daa62fc59061d22133a7cbc7c1572e` (`TERMINAL_BENCH_4_SOURCE_COMMIT`). Override the checkout with `BENCH_TERMINAL_BENCH_4_TASKS_DIR=<repo root>`.
- Harbor dataset id: `terminal-bench/terminal-bench@4.0.0` (<https://hub.harborframework.com/datasets/terminal-bench/terminal-bench/4>).
- License: Apache-2.0 (repository `LICENSE`).
- The checkout contains 66 task manifests. This harness exposes **55** of them, see the exclusion below.

## Task format

Each task ships `task.toml`, `instruction.md`, `environment/Dockerfile` (agent image), `tests/Dockerfile` and `tests/test.sh` (verifier image). Only the manifest fields the harness reads are validated (`schema.ts`): agent and verifier timeouts, agent and verifier resources (`cpus`, `memory_mb`, `storage_mb`, `gpus`, `gpu_types`, `allow_internet`, `env`), `artifacts`, `verifier.collect`, `verifier.env` and `metadata.category`. All 66 manifests declare `verifier.environment_mode = "separate"`.

## Execution model

Unlike 2.1, the verifier does not run inside the agent container.

1. Agent sandbox is created from the task's prebuilt agent image with the task's `cpus`, `memory_mb`, GPU and `environment.env`. Internet is always enabled so the agent CLI can reach OpenRouter, and tasks that declare `allow_internet = false` record `agentNetworkForced: true` in sample metadata.
2. The ori agent runs against `/instruction.md` with the task's `agent.timeout_sec`.
3. `verifier.collect` hooks run in the agent sandbox. Hooks that target a compose sidecar service fail the sample.
4. `/logs/artifacts` plus every declared artifact path is bundled into a tarball, downloaded, and extracted at the same absolute paths in a fresh verifier sandbox built from the task's verifier image with `verifier.environment` resources (falling back to the agent resources) and `verifier.env`.
5. `/tests/test.sh` runs with `verifier.timeout_sec`. Reward is read from `/logs/verifier/reward.txt` and scored 1 only when the reward is 1, otherwise 0 (`harbor/reward.ts`).

Per-task sandbox timeouts are the task's declared timeouts plus a fixed margin, so an 8 hour task gets an 8 hour agent sandbox.

## GPUs

Three tasks declare one H100 each: `fp8-rmsnorm-gemm`, `jax-speedrun-gpu`, `math-eval-grader`. `toModalGpu` maps `gpus`/`gpu_types` to the Modal `gpu` string (`"H100"`, or `"H100:2"` for multiple) and refuses tasks that request GPUs without naming a type. `storage_mb` is parsed but not enforced because the Modal sandbox API used here has no disk-size parameter.

## Images

4.0 tasks ship Dockerfiles with local-context `COPY` steps rather than prebuilt image names, and the Modal JS SDK rejects those Dockerfiles. Images are built once per pinned commit with the Modal Python SDK (which supports a local build context) and stored in Modal's image store, so no external registry is involved:

```bash
pip install modal
MODAL_TOKEN_ID=... MODAL_TOKEN_SECRET=... \
  python3 scripts/build-terminal-bench-4-images.py --tasks-dir <checkout>/tasks [--task <id>] [--concurrency 4] [--dry-run]
```

The script writes `image-ids.json` (`{ sourceCommit, images: { <taskId>: { agent, verifier } } }`) with Modal image IDs. `images.ts` validates it at load time and refuses a map whose `sourceCommit` differs from `TERMINAL_BENCH_4_SOURCE_COMMIT`; the solver fails a sample whose task has no entry. Sandboxes are created with `imageKind: "modal-image-id"`, which resolves the ID with `images.fromId` before layering the agent install steps. Images built into the `terminal-bench-4-images` Modal app are visible to the run-time app because Modal image IDs are workspace-scoped.

## Excluded tasks

Eleven tasks use `environment/docker-compose.yaml` and need more than one container. The Modal sandbox abstraction here is single-container, so they are excluded from the dataset and reported by `listComposeTaskIds`:

`ctr-optimization`, `cumulative-layout-shift`, `freight-dispatch-shift`, `heat-pump-warranty`, `intrastat-meldung`, `kv-live-surgery`, `legacy-utility-triage`, `live-database-cutover`, `medical-claims-processing`, `nextjs-performance`, `payments-pipeline-fix`.

A score over the 55 runnable tasks is not an official Terminal-Bench 4.0 number.

## Sandbox lifetimes

The agent sandbox outlives `maxAgentTimeoutSec` by the sum of collect-hook timeouts plus the artifact bundle, transfer and extract budgets plus a fixed margin (`agentSandboxTimeoutSec`). The verifier sandbox is created before artifact transfer, so its lifetime covers the same artifact budgets plus `maxTestTimeoutSec` (`verifierSandboxTimeoutSec`).

## Artifact semantics

Harbor empties directory artifact targets in the verifier before uploading the agent's copy. `artifactExtractCommand` mirrors this: any declared source that appears as a directory in the bundle is removed on the verifier before extraction, so files the agent deleted do not survive. Excluded patterns are simply absent from the bundle; verifiers that need a pristine copy of an excluded file restore it themselves (see `vpp-loss-divergence`).

## Config options

Same agent options as `terminal_bench` (`TerminalBenchOptionsSchema`).
