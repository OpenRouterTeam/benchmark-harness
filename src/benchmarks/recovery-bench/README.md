# Recovery-Bench

[Recovery-Bench](https://github.com/letta-ai/recovery-bench) (Letta) measures whether an agent can finish a Terminal-Bench 2.0 task after a previous agent already failed at it and left the environment in a damaged state. Each sample starts from a failed trajectory, replays that trajectory's shell commands into a fresh copy of the original task container, then hands the recovery agent the task plus (optionally) the failed attempt's history. Scoring is the unmodified Terminal-Bench verifier for the task.

This integration exists to run paired rescue experiments: from the identical failed checkpoint, compare the original model continuing, the same model with the recovery prompt, a fixed stronger model, and router-selected models, holding the prior context and remaining budget constant.

## Source, license and pinning

- Recovery-Bench: <https://github.com/letta-ai/recovery-bench>, MIT license, pinned to commit `c5f83f2ba4f882a9b544c7bf0fa9be1bc3859c78` (`RECOVERY_BENCH_SOURCE_COMMIT`).
- Failed trajectories: the bundled run `runs/initial-claude-haiku-4-5-20251001-20260303_194859` (`RECOVERY_BENCH_INITIAL_RUN`), produced upstream by Terminus 2 driving `claude-haiku-4-5`. The trajectory files are stored in the upstream repository via Git LFS and are **not committed here**. The committed `recovery-bench-manifest.json` records, per trial, the task id, trial directory, SHA-256 and byte length of `agent/trajectory.json`, the byte size of the full-history recovery instruction, and the replay command count. At run time each trajectory is fetched from `media.githubusercontent.com` at the pinned commit through the shared cached-file fetcher and rejected unless both length and digest match the manifest.
- Task definitions and verifiers: Terminal-Bench **2.0**, <https://github.com/laude-institute/terminal-bench-2>, pinned to commit `69671fbaac6d67a7ef0dfec016cc38a64ef7a77c` (`TERMINAL_BENCH_2_SOURCE_COMMIT`). Recovery-Bench's trajectories were recorded against 2.0 task text and images, which differ from the Terminal-Bench 2.1 source used by `terminal_bench`, so this benchmark keeps its own pinned checkout (`BENCH_RECOVERY_BENCH_TASKS_DIR` overrides the checkout location).

Regenerate the manifest from a local clone with LFS objects pulled:

```bash
bun scripts/build-recovery-bench-manifest.ts --checkout /path/to/recovery-bench
```

## Trial selection

The manifest follows upstream `get_unsolved_tasks`: a trial is included when its `result.json` exists, parses, and reports `verifier_result.rewards.reward <= 0` (or no reward), and its `agent/trajectory.json` exists and parses as ATIF. That yields 64 trials at the pinned commit. Upstream additionally refuses to run any trial whose full-history recovery instruction exceeds 64,000 bytes, so at dataset time trials with `fullContextBytes > maxInstructionBytes` (default 64,000) are dropped, leaving 56 runnable trials in `full` mode. Raising `maxInstructionBytes` includes the 8 larger trials but is not comparable to upstream numbers. `taskSubset` accepts task ids or trial directory names.

Sample ids are `recovery_bench-<trialDir>`, so the same failed checkpoint has the same id across models and context modes.

## Sandbox lifecycle

1. Fetch and verify the trajectory, parse it (ATIF v1.6), and build the recovery instruction. Both happen before any sandbox is created, so malformed traces and oversized instructions fail cheaply.
2. Create a Modal sandbox from the task's 2.0 `docker_image` with the task's CPU and memory settings and the agent runtime baked in.
3. Replay the failed attempt: every `terminus_2_command` keystroke string is run as `bash -lc <keystrokes without trailing newline>`. Control-only keystrokes such as `C-c` are not run. A command immediately followed by a control keystroke is skipped, because the original agent interrupted it (usually a server or a hung process). Failures and timeouts do not abort replay. Each command gets `replayCommandTimeoutSec` (default 15 s), and the sandbox lifetime is extended by `executable commands × replayCommandTimeoutSec` so a slow replay cannot eat the agent or verifier budget. This is a faithful port of upstream `pipeline.py`, including its known limitation that interactive programs are not reproduced.
4. Upload the recovery instruction over `/instruction.md`, run the configured agent via `ori`, then run the task's verifier and read `/logs/verifier/reward.txt`.
5. Destroy the sandbox in `finally`.

## Context modes

`messageMode` selects how much of the failed attempt the recovery agent sees. All three use the upstream preamble verbatim.

| mode | prior context block |
| --- | --- |
| `full` (default) | Every step of the failed trajectory as `[ROLE]: message` text |
| `summary` | A summary of those steps produced by `summaryModel` through the normal harness model path, or the fixed upstream fallback sentence when no `summaryModel` is set or the summary call fails |
| `none` | No prior context, only the preamble and the original task |

When a summary model is invoked, its tokens, cost and latency are added to the sample's usage and `generationTimeMs`, and recorded separately as `summaryCost` and `summaryTimeMs` in metadata.

## Config options

All `terminal_bench` options (agent, `agentReasoningEffort`, `modalEnv`, `taskSubset`, `maxAgentTimeoutSec`, system prompt and tool controls) apply. Additional options:

| option | type | default | description |
| --- | --- | --- | --- |
| `messageMode` | `"full"` \| `"summary"` \| `"none"` | `"full"` | Prior-context condition, see above |
| `summaryModel` | string | unset | Model used to summarize the failed attempt in `summary` mode |
| `replayCommandTimeoutSec` | number | `15` | Per-command timeout during replay |
| `maxInstructionBytes` | integer | `64000` | Upper bound on the recovery instruction. Also filters the manifest |

Set `reasoningEffort` and `agentReasoningEffort` explicitly. Do not set `maxTokens`. Use `chunkSize: 1` for distributed runs so an instance failure loses one sample.

## Scoring and metadata

`terminalBenchScorer`: reward `>= 1` is Correct, anything else Incorrect. Per-sample metadata includes `reward`, `testOutput`, `messageMode`, `recoveryInstructionBytes`, `summaryUsed`, `summaryFellBack`, `summaryCost`, `summaryTimeMs`, `priorModel`, `priorMessages`, `replayAttempted`, `replayFailed`, `replaySkippedInterrupted`, `replayTotalExecutable`, the agent CLI metadata (generation ids, turns, tool calls) and the pinned commits and trajectory digest.

## Run

```bash
OPENROUTER_API_KEY=... bun run bench -- --benchmark recovery_bench --model deepseek/deepseek-v4 --limit 5 --concurrency 2
```

## Limitations and calibration status

- Only the bundled Haiku failures are available. Conclusions about a model recovering from its **own** failures need that model's failed Terminal-Bench 2.0 traces added to the manifest; the manifest builder accepts any Recovery-Bench-format run directory.
- The manifest keeps every unsolved Haiku trial. Filtering to "repeated failure with no measurable progress" is a downstream selection on `priorMessages`, `replayFailed` and the fetched trajectory, not something this dataset does.
- Replay executes commands non-interactively and cannot reproduce interactive sessions, background servers or timing-dependent state, matching the upstream limitation.
- No run against a published Recovery-Bench number has been performed yet, so this integration is **not calibrated**. Upstream reports results for its own agent and model set; the closest check is running `full` mode with `agent: "claude"` on `anthropic/claude-haiku-4.5` and comparing to the upstream Haiku recovery rate.
