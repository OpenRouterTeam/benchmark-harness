# Decisions

Closed-set typed-decision benchmark for the OpenRouter `/api/alpha/decisions` route (TypeSafe Jev and other System One style models) with a matched generative-LLM comparator arm. Every sample is one `state` plus one typed question (`noul`, `choice`, or `score`) whose answer set is fixed by the dataset labels. Both arms are scored against the same closed answer set as a probability distribution, so accuracy, NLL, Brier, ECE, selective-prediction, and cost metrics are comparable across arms without a categorical mismatch.

## What is held constant and what varies

Held constant per run: the task, language, variant, closed label set, ground truth, and scoring. The only intentional difference between arms is the output contract adapter (native typed probabilities for `arm: "decision"`, a strict `{"answer", "confidence"}` JSON completion at `temperature: 0` for `arm: "llm"`). One factor varies per run via `variant`.

| variant | what changes | mechanism isolated |
| --- | --- | --- |
| `base` | nothing | reference cell |
| `shuffled` | seeded permutation of `choice` options or `score` levels (gold index remapped) | option-order sensitivity |
| `reversed` | `choice` options or `score` levels listed in reverse order | ordinal-direction coupling |
| `distractor` | one never-correct option added to `choice` criteria | option-interaction / probability leakage |
| `no_criteria` | `noul` criteria removed, `choice` criteria set to `null` (labels only), `score` unchanged | reliance on criteria text |
| `state_object` | state sent as an object with named fields instead of a string | structured-state handling |
| `evidence_removed` | state replaced with an unavailability placeholder, gold is unknowable | abstention and overconfidence |

For `evidence_removed` a sample is counted correct only when the emitted confidence is below `CONFIDENCE_THRESHOLD` (0.9) and the output is well formed, since no answer is knowable.

## Tasks, sources, and licenses

All datasets are streamed from Hugging Face (never materialized). Each row is validated with a Zod schema before use, and rows with unknown labels or inconsistent answer indexes are rejected rather than repaired. Sample IDs are `decisions-<task>-<language>-<variant>-<rowIndex>`.

| task | primitive | HF dataset (config, split) | labels | license as declared on the card |
| --- | --- | --- | --- | --- |
| `banking77` | choice | `mteb/banking77` (`default`, `test`) | 77 intents | MIT on the mteb card, original PolyAI/banking77 CC-BY-4.0 |
| `massive_intent` | choice | `mteb/amazon_massive_intent` (`<language>`, `test`) | 60 intents | Apache-2.0 on the mteb card, original MASSIVE CC-BY-4.0 |
| `dbpedia_14` | choice | `fancyzhx/dbpedia_14` (`dbpedia_14`, `test`) | 14 classes | CC-BY-SA-3.0 |
| `boolq` | noul | `google/boolq` (`default`, `validation`) | true / false | CC-BY-SA-3.0 |
| `prompt_injection` | noul | `deepset/prompt-injections` (`default`, `test`) | true / false | Apache-2.0 |
| `sst5` | score | `SetFit/sst5` (`default`, `test`) | 5 ordered levels | not declared on the card (Stanford Sentiment Treebank), verify before publication |
| `mmlu_pro` | choice | `TIGER-Lab/MMLU-Pro` (`default`, `test`) | up to 10 lettered options | MIT |
| `xnli` | choice | `facebook/xnli` (`<language>`, `test`) | entailment / neutral / contradiction | not declared on the card, XNLI is CC-BY-NC-4.0 (non-commercial) |

`PolyAI/banking77` is not used directly because Hugging Face rejects its dataset script. `massive_intent` and `xnli` take the `language` option as the HF config name. Restricted or non-commercial rows must not be committed to this repository.

## Arms

- `arm: "decision"` posts a `DecisionsRequest` to `/api/alpha/decisions` through a separate non-streaming client. It preserves endpoint pinning (`X-OR-Endpoint-Id`), provider `only` / `ignore` / `allow_fallbacks`, session ID, cache salt per session / epoch / call / retry attempt, trace headers, generation ID, latency, and usage. Responses are validated with `DecisionsResponseSchema`, then checked against the answer invariants (answer-set equality, type match, probability bounds and normalization, choice membership, argmax consistency, legend consistency, score as weighted mean). Violations are recorded on the outcome and count as hard violations in scoring.
- `arm: "llm"` sends a system prompt plus a user prompt listing the exact labels through the standard OpenRouter model layer at `temperature: 0`. `maxTokens` is deliberately not set. The completion must end in a JSON object `{"answer": "<label>", "confidence": <0..1>}`. The parsed answer is mapped to a distribution that places `confidence` on the chosen label and the remainder uniformly on the others. Unparsable output or a label outside the closed set is a hard violation and keeps an empty distribution (unknown stays unknown). Missing confidence or confidence below uniform is a soft violation that stays measurable.

## Metrics

Per-sample: correctness, argmax, confidence, probability on gold, Brier, NLL, ranked probability score and absolute level error (ordinal `score` tasks only), latency, tokens, cost, and violations. Run-level groups are keyed by task, language, variant, and arm and report accuracy, NLL, Brier, ECE, mean confidence, confidence bias, coverage and accuracy at the confidence threshold, confident error rate, selective accuracy at 50% and 80% coverage, AURC, RPS, score MAE, evidence-removed mean confidence and confident rate, latency p50 / p95, mean tokens, total cost (omitted when no cost is known), and violation rates.

## Config options

Passed via `--solver-config '<json>'` on the CLI.

| option | type | default | description |
| --- | --- | --- | --- |
| `arm` | `"decision"` \| `"llm"` | `"decision"` | Which output-contract adapter to run. |
| `task` | task ID above | `"banking77"` | Dataset and primitive. |
| `variant` | variant above | `"base"` | Single-factor perturbation. |
| `language` | string | `"en"` | HF config for `massive_intent` and `xnli`, recorded in IDs for all tasks. |

Standard inference overrides (`endpointId`, `providerOnly`, `providerIgnore`, `allowFallbacks`, `reasoningEffort`, `timeoutMs`) apply to both arms where meaningful. The Decisions arm ignores `reasoningEffort`.

## Running

```bash
OPENROUTER_API_KEY=... bun run bench -- --benchmark decisions --model typesafe/jev-1.13 --limit 20 --solver-config '{"task":"boolq"}'
OPENROUTER_API_KEY=... bun run bench -- --benchmark decisions --model openai/gpt-5-mini --limit 20 --reasoning-effort low --solver-config '{"task":"boolq","arm":"llm"}'
```

## Reference numbers

No published Jev numbers on these exact splits are available to reproduce. Dataset cards report the following for other model classes on the same splits and label sets, which bound what a matched closed-set run should approach: Banking77 fine-tuned encoder accuracy of roughly 93% (PolyAI card), BoolQ validation accuracy of roughly 80% to 90% for fine-tuned encoders (SuperGLUE), MMLU-Pro accuracy varies widely by model and is reported per model on the TIGER-Lab card. Treat these as Reported values from third parties, not as reproduced targets.

Live smoke runs against `typesafe/jev-1.13` through `https://openrouter.ai/api/alpha/decisions` (returned model `typesafe/jev-1.13-20260917`, provider `TypeSafe`) completed for `boolq` (10 samples, 7/9 scored correct, one sample skipped after the retry budget on upstream 503/529), `banking77` (77-option `choice`), `sst5` base and `reversed` (5-level `score`), and `banking77` `evidence_removed`. Every scored answer passed the response invariants with zero hard violations, `usage.output_tokens` was non-zero (22 for a single `noul`), and per-sample latency was 100 ms to 600 ms. These are smoke checks, not scores; sample counts are far too small to compare against the reference numbers above.
