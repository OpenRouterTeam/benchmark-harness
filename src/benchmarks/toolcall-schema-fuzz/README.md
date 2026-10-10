# toolcall_schema_fuzz

Serving-stack check, not a capability benchmark. Each sample already contains the expected tool call and its result in the history and asks the model to repeat the call with the same arguments, so a failure points at the serving stack (chat template, tool parser, constrained decoder, argument re-serialization) rather than the model. Pin `providerOnly` or `endpointId` and state the pin alongside any number.

## Source & license

All data is synthetic and written by OpenRouter; there is no customer traffic in it.

- `grid.ts` enumerates every primitive, ordered type pair, enum, const, `{}`, anyOf/oneOf branch, validation keyword (pattern, format, length, numeric and item bounds, prefixItems, additionalProperties schemas, recursive `$ref`) and every object/array/`$ref`/nullable/allOf wrapper up to depth 2, strict and non-strict, in four scenarios: a single call, a replay after an earlier tool turn, beside a near-duplicate distractor tool, and as two parallel calls.
- `realistic/wordings.json` rewords every grid shape as a realistic tool, keys, values and request. It was written offline by an LLM rewriter (`cli/reword.ts`, writer and checker models recorded in the file) and accepted only when a second model reproduced the call. The arguments are invented, not sampled from real requests.
- `regressions.json` is where `cli/discover.ts` records confirmed, shrunk failures, each with its closest passing control. It is empty today.

All of it is released under this repository's license. Sample ids are stable; never rename one.

## Evaluation method

- Temperature is fixed at 0. Requests go to `/chat/completions` with the case's system message, replayed history and tools; `parallel_tool_calls` is set for the parallel scenario.
- `scorer.ts` validates each returned call against the exact schema sent (`json-schema.ts`), then compares it with the expected call. Failures are `no_tool_call`, `call_count`, `unknown_tool`, `wrong_tool`, `invalid_json`, `schema_violation` (wrong type or shape), `value_mismatch` (schema-valid but altered), `request_rejected` (the stack returned HTTP 400 or 422 for the request, e.g. an unsupported schema keyword) and `truncated` (skipped, not failed). Other 4xx responses (auth, unknown model) abort the run.
- `cli/report.ts` prints per-case stability and per-construct and per-scenario totals from result parquet files.

## Running

```sh
bun install
OPENROUTER_API_KEY=... bun run bench --benchmark toolcall_schema_fuzz \
  --model openai/gpt-4.1 --solver-config '{"providerOnly":["openai"]}' --concurrency 32
bun src/benchmarks/toolcall-schema-fuzz/cli/report.ts bench-results/<run>.parquet
```

Results go to `bench-results/`. Pass `generator` in `--solver-config` to pick a case set.

## Config options

- `generator`: `realistic` (default, 2668 samples, comparable across runs), `sentinel` (coverage-preserving subset of `realistic` plus any regressions, meant for many epochs) or `regressions` (confirmed shrunk failures; errors while `regressions.json` is empty).
