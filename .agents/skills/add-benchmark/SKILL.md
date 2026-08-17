---
name: add-benchmark
description: Implement a new benchmark in the harness catalog — feasibility checks, dataset streaming and recordToSample, solver, scorer, config schema, metadata, registry and CLI wiring, tests, and a calibrated smoke run. Also lists the harness-level mistakes that silently corrupt scores (caching, answer extraction, inference-parameter propagation, judge variance, sandbox timeouts, media fetchability). Use when adding any eval to this repo.
---

# Add a benchmark to the catalog

A benchmark is a `Benchmark` value (`src/benchmarks/types.ts`) that supplies three
Effect layers, `Dataset`, `Solver` and `Scorer`, plus its id, temperature and
default epoch count. The runner in `src/harness/run.ts` does everything else.
Your job is to produce those layers, register them, and prove the resulting
accuracy is real.

## 1. Establish feasibility before writing code

Answer all five. If any answer is unknown, stop and resolve it first.

1. **Mode.** `chat` means one prompt in, one completion out, graded by a pure
   scorer. Use `defineChatBenchmark` (`src/benchmarks/define-chat-benchmark.ts`).
   `agentic` means a multi-turn tool loop, user simulator, sandbox, or a
   judge or panel DAG. Write the `Benchmark` by hand and give it its own
   directory, as `src/benchmarks/terminal-bench/` and
   `src/benchmarks/tau3-bench-banking/` do.
2. **Dataset.** Confirm the exact source, config and split resolve, and that the
   fields you need are present in the rows you will actually read. Record the
   dataset provenance and its license in a `README.md` next to the benchmark,
   as `src/benchmarks/vgi-bench/README.md` does. Do not add a benchmark whose
   license forbids the use.
3. **Scoring.** Deterministic string or choice matching, state inspection of a
   sandbox or environment, or an LLM judge. Prefer the earliest of those three
   that the task admits.
4. **Closest existing benchmark.** Copy its shape. `gpqa` for multiple choice,
   `mmlu-pro` for multiple choice with a dynamic prompt,
   `search/*` for retrieval plus judge, `terminal-bench` for sandboxed agents,
   `vgi-bench` for media inputs.
5. **Published target score.** Record the published number for at least one
   model you can run. Without it you cannot tell a working benchmark from a
   broken one.

## 2. Files you touch

| Purpose | Location |
| --- | --- |
| Dataset mapping and benchmark definition | `src/benchmarks/<slug>.ts` or `src/benchmarks/<slug>/` |
| Config schema and its union member | `src/benchmarks/benchmark-config.ts` |
| Default epochs, temperature, user model | `src/benchmarks/benchmark-meta.ts` |
| Catalog entry | `src/benchmarks/registry.ts` |
| CLI dispatch for the config | `src/cli/index.ts` |
| Tests | `*.test.ts` colocated with the code |

## 3. Dataset and `recordToSample`

Stream the dataset. Never materialize it. `makeHfDatasetLayer`
(`src/datasets/huggingface.ts`) pages the HuggingFace rows endpoint and exposes
the result as a `Stream` of `Sample`, so memory stays flat regardless of dataset
size. If you add a non-HF source, keep it a stream with the same page-and-retry
shape rather than a single fetch that buffers every row.

`recordToSample` must be pure and deterministic in `(record, index)`. It
validates every field it reads out of the row and throws a typed error on
surprise, as `gpqa.ts` does with its `asString` guard. Sample ids are stable
across runs, so `<benchmark_id>-<index>`.

Media inputs go through `contentParts` (`src/harness/core.ts`). Every asset must
be fetchable by every provider under test, which upstream dataset URLs frequently
are not. Mirror the assets to storage you control and reference the mirror, as
`scripts/mirror-vgi-bench-media.ts` does, or inline the bytes as base64. A media
URL that one provider can reach and another cannot produces a score difference
that looks like a model difference.

## 4. Solver

Every inference parameter comes from the parsed config, never from a literal in
the solver body. `InferenceOverrideSchema` in `benchmark-config.ts` is the full
set, including `temperature`, `maxTokens`, `reasoningEffort`, `timeoutMs`,
`endpointId`, `providerOnly`, `allowFallbacks` and `sort`. Spread it through
`definedValues` so an unset option stays absent from the request and the API
side default applies. Sending an explicit default is not the same as sending
nothing.

Benchmarks with a fixed temperature take `FixedTemperatureBenchmarkBaseSchema`
and keep the temperature in their metadata, as `GPQA_META` does. Do not let a
config override silently replace a temperature the published score depends on.

Endpoint pinning and provider routing must survive your solver.
`endpointId` becomes the `X-OR-Endpoint-Id` header and suppresses provider
sorting, so verify a pinned run actually reaches the pinned endpoint rather than
assuming the header was forwarded.

For multi-turn solvers, pass `reasoning_details` back on every subsequent
request. `src/providers/openrouter-model.ts` replays them when the assistant message carries
them, which is what preserves encrypted reasoning across turns. Dropping them
degrades reasoning models specifically, and only on multi-turn benchmarks.

Run the agent loop against the `Model` service so requests carry the harness
headers. A solver that shells out to an external agent binary bypasses caching,
session attribution and generation-id collection at once.

## 5. Scorer

Scorers are pure. Given `state` and `target` they return a `ScoreValue`, the
extracted answer, and an explanation, as `src/benchmarks/scorers/mcq/scorer.ts` does.

Answer extraction is where accuracy is usually lost. Reuse
`extractMcqAnswer` (`src/benchmarks/scorers/mcq/extract.ts`) for letter answers rather than
writing a new regex. It already handles bold and LaTeX wrappers, `\boxed{}`,
parenthesized letters and full-width and non-Latin letter forms. If your task
needs new extraction, add the pattern there with a test for the response shape
that motivated it, and always record the extracted answer on the score so a
failed extraction is distinguishable from a wrong answer in the transcript.

For multiple choice, shuffle the options with `seededPermutation`
(`src/benchmarks/scorers/mcq/shuffle.ts`) keyed on the sample index, and derive the target
letter from the permutation. Datasets that store the correct answer in a fixed
field, GPQA among them, otherwise reward position bias instead of knowledge, and
the shuffle must be seeded so the run is reproducible.

Judge-based scoring pins the judge model, temperature and reasoning effort in
`JudgeConfig` (`src/judge/judge.ts`) and constrains the verdict with a strict
JSON schema plus a parse fallback. Treat the judge as part of the benchmark
definition. Changing it changes the benchmark. Measure judge run-to-run variance
on a fixed set of completions before you attribute any score delta to the model
under test.

## 6. Config schema, metadata, registry, CLI

Every option the benchmark accepts is declared in its Zod schema and reachable
from the CLI. `src/cli/index.ts` rejects unknown `--solver-config` keys against
the schema, so an option you forget to declare is unreachable rather than
silently defaulted.

1. Add `<Bench>OptionsSchema` and `<Bench>BenchmarkConfigSchema` in
   `benchmark-config.ts` with `benchmarkId: z.literal('<id>')` and the right
   base schema, then add it to the run-config union.
2. Add `<BENCH>_META` in `benchmark-meta.ts` and read `defaultEpochs` and
   `temperature` from it rather than repeating the literals.
3. Add the benchmark to `BENCHMARKS` in `registry.ts`.
4. Add the config construction case in `src/cli/index.ts`, and a
   `BenchmarkCliPlugin` if the benchmark needs argv of its own.
5. Set `degradeSolverErrors: true` only for agentic benchmarks where a failed
   rollout is a legitimate zero, as `terminal-bench` and `deep-swe` do.

## 7. Run-level correctness checks

These are harness-level and independent of the task. Verify each one on a real
run before reporting a score.

- **Cache salting.** Response caching is keyed by the salt built in
  `buildResponseCacheSalt` (`src/runtime/response-cache.ts`) from session id,
  epoch, retry attempt and per-call salt. Epochs of the same sample, and
  repeated calls within one sample, must land on different salts, otherwise the
  second call returns the first answer and variance collapses to zero. Any new
  request path you add attaches the cache headers, and any solver that issues
  more than one call per sample wraps each in `withCallCacheSalt`.
- **Prefix reuse.** Distinct samples that share a long identical prompt prefix
  are the other way this shows up. Confirm the sample-varying content is inside
  the request, not appended after a cached prefix boundary.
- **Session id.** Requests carry `x-session-id`, which
  `makeOpenRouterModelLayer` sets from the run's `sessionId`. Without it a run
  cannot be reconciled afterwards.
- **Real generation id.** On a cache hit the response id is not the generation
  that produced the text. Resolve the true source through
  `GenerationResolver` (`src/runtime/generation-resolver.ts`) so cost and usage
  attribution stay correct. Server-tool calls have the same problem.
- **No truncation.** Anything you need in the transcript, completion text,
  reasoning, tool calls and errors, must be persisted untruncated. Check the
  written result rather than the console output.
- **Provider variance.** The same model on two providers scores differently.
  Pin `providerOnly` or `endpointId` for any comparison you intend to publish,
  and state the pin alongside the number.
- **Sandbox timeouts.** Sandboxed benchmarks derive per-task agent and verifier
  timeouts from the task definition, as `terminal-bench/dataset.ts` does. A
  global timeout scores slow tasks as failures, and the failure looks like a
  model failure.

## 8. Tests

Colocate `*.test.ts`. Keep them deterministic and network-free. Cover, at
minimum, `recordToSample` against a real row shape including the fields you
guard, the scorer against the response formats you expect and at least one you
do not, the shuffle-to-target derivation, and the metadata-to-registry
agreement on id and `defaultEpochs`.

## 9. Smoke run and calibration

```bash
bun run bench --benchmark <id> --model <model> --limit 20 --concurrency 4
```

Read the per-sample results, do not just read the aggregate. Look for scores of
zero with a non-empty completion, which means extraction failed, identical
completions across epochs, which means cache salting failed, and truncated
transcripts. Then run enough samples to compare against the published target you
recorded in step 1. A benchmark that does not land near a published number for a
known model is not finished.

## 10. Validation

```bash
bun run format:check && bun run check && bun run typecheck && bun test && bun run build
```
