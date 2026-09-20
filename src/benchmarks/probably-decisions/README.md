# probably_decisions

Evaluates a model as the **judgment step** of a Trust and Safety decision program written in [Probably](https://probably-lang.southpolesteve.workers.dev/), and optionally as the **research agent** that assembles the dossier the program judges.

## How a sample runs

1. `programs.ts` holds the fixed Probably program (`sentinel_case_v1`). It encodes the Sentinel scanner authority rules as `feels` / `match` judgments: compromised-key gate, static-attribute-only leads hold, load versus abuse, two-signal corroboration, remedy selection and the `account_ban` escalation bar. The program is the benchmark contract. A change to it is a benchmark behaviour change.
2. `probably/runtime.ts` interprets the program. Every semantic judgment is delegated to a `Provider.judge(value, labels)` call, which `judge.ts` satisfies in one of two ways. Chat models are prompted for a JSON probability distribution over the lettered labels. Decisions models (`typesafe/*`, e.g. `~typesafe/jev-latest`) are called natively through `POST /api/alpha/decisions` with a single `choice` question whose criteria are the labels, and the returned `probabilities` are mapped back to labels. Routing is by model id (`isDecisionsModel` in `src/providers/decisions-client.ts`), no extra flag.
3. **judgment mode** feeds the redacted dossier plus evidence sections straight into `input()`. **research mode** first runs a tool loop (`list_evidence_sections`, `read_evidence_section`, `submit_dossier`) with the model under test, then judges the submitted dossier with the configured `judgeModel` (default: the same model). Holding the program fixed lets a wrong action be attributed to research or to judgment.
4. `scorer.ts` compares the program's final `print` to the gold action and records per-decision branch agreement, a Brier score on the chosen-branch distributions, enactment agreement (`hold` versus any restriction), evidence coverage and gold-fact recall (research mode only).

The primary score is `action_accuracy`. Run-level metrics also include `enactment_agreement`, `macro_recall`, `hold_precision`, `brier`, `branch_agreement`, `evidence_coverage`, `fact_recall`, `run_failure_rate` and `recall_<action>`.

## Dataset

`__fixtures__/sentinel-sample.jsonl` is a 102-record stratified sample (every denied and reverted case, plus 12 approved cases per action) built from OpenRouter's internal Sentinel ban-candidate queue. It ships in the repository so the benchmark runs without network access. A larger build of the same generator can be pointed at with `datasetUrl` (`file:` or `https:`).

**Provenance.** Each record is one Sentinel suggestion (a scanner-proposed restriction on one or more accounts) read through the Sentinel ban-candidates CLI, joined with the human review outcome. The gold action is derived from the reviewed target state:

- `approved` targets keep the proposed remedy (`frontier_block`, `inference_block`, `throttle`, `key_revocation`, `account_ban`).
- `denied` and `reverted` suggestions map to `hold` (file for review, do not enact).

`gold.facts` are short evidence tokens (client fingerprints, model slugs, bucketed counts) that a research agent should surface in its dossier. They are matched by case-insensitive substring.

**Redaction.** The generator is field-aware and deterministic. Emails, names, Clerk IDs, API keys, IPv4 addresses, Slack references, internal links and suggestion UUIDs are removed or pseudonymised (`<account>`, `<ip>`, `case-<sha256 prefix>`), numeric identifiers become `<id>`, and record ids are a SHA-256 prefix of the original suggestion id. Gold facts that are numeric-only or contain an identifier are dropped. `dataset.test.ts` re-asserts these invariants over every string field of the bundled fixture.

**Licensing.** Internal OpenRouter data. Not for redistribution outside the organisation. Generated Slack summaries were used only as leads, never as labels. Nothing in this benchmark files, approves, enacts or reverts a restriction. Those paths stay behind the Sentinel ban-candidates API.

## Config

```jsonc
{
  "benchmarkId": "probably_decisions",
  "model": "openai/gpt-5",
  "reasoningEffort": "medium",
  "mode": "judgment", // or "research"
  "judgeModel": "openai/gpt-5-mini", // research mode, optional
  "maxResearchSteps": 16,
  "program": "sentinel_case_v1",
  "datasetUrl": "file:///path/to/probably-decisions.jsonl", // optional
}
```

Temperature is fixed at 0. `maxTokens` is intentionally never set. Use `chunkSize: 1` for research mode.

### Jev and other Decisions models

- **judgment mode**: set `model` to `~typesafe/jev-latest`. Every `feels` / `match` is one Decisions request. Temperature and reasoning settings do not apply to the Decisions endpoint. Provider preferences (`providerOnly`, `providerIgnore`, `allowFallbacks`, `sort`) are forwarded as `provider`.
- **research mode**: the tool loop needs a chat model, so `model` must be a chat model and Jev goes in `judgeModel`. A Decisions model as `model` in research mode fails at startup with an explanatory error.
- Judge transcripts still record the lettered prompt and the returned distribution as JSON, so traces from chat and Decisions judges are comparable.
