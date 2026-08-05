import { GPQA_META, TAU3_BENCH_BANKING_META } from "./benchmark-meta";
/**
 * First-party eval manifests (M2 of the unified-manifest migration).
 *
 * Each registered benchmark gains an {@link EvalManifestV1} with `builtin`
 * bindings naming its TS implementation. The manifests are the durable,
 * serializable description; `benchmark-meta` literals and Mission Control
 * constants are asserted equal to them by tests today and derived from them
 * once every benchmark is covered.
 *
 * Starting set: gpqa (simplest chat benchmark) and tau3-banking (hardest:
 * multi-role, builtin dataset/solver/scorer, per-benchmark options). The
 * remaining 14 follow mechanically once this shape survives review.
 */
import type { EvalManifestV1 } from "./eval-manifest";
import { EVAL_MANIFEST_API_VERSION } from "./eval-manifest";

export const GPQA_MANIFEST: EvalManifestV1 = {
  apiVersion: EVAL_MANIFEST_API_VERSION,
  id: GPQA_META.id,
  name: "GPQA Diamond",
  version: "1",
  /*
   * Deliberately NOT `gpqa-diamond.public.v1` (the Compass catalog's name for
   * the simple-evals reference protocol, n_repeats=4): our first-party series
   * runs 10 epochs with a per-record seeded shuffle, which is a different
   * comparability series and must not masquerade as the public one.
   */
  protocolId: "gpqa-diamond.openrouter.v1",
  dataset: {
    kind: "hf",
    dataset: "nmayorga7/gpqa_diamond",
    config: "default",
    split: "train",
    /* Captured 2026-08-05; the loader fails closed if upstream moves. An
     * upstream dataset change is a new comparability series — re-pin here
     * (which changes the identity digest) rather than un-pinning. */
    revision: "c63e9ba02dc3da4c698e2a8485551b35041c3900",
    // The builtin solver renders the full MCQ prompt; these fields document
    // the record shape for tooling rather than driving a spec solver.
    inputField: "Question",
    targetField: "Correct Answer",
  },
  models: [{ role: "candidate", overridable: true }],
  solver: { kind: "builtin", ref: "gpqa-solver" },
  scorer: { kind: "builtin", ref: "mcq" },
  sampling: { temperature: GPQA_META.temperature },
  epochs: { default: GPQA_META.defaultEpochs, reducer: "mean" },
  execution: { tier: "trusted-local", drivers: ["process"] },
  reports: [
    {
      key: "score",
      label: "Accuracy",
      type: "scalar",
      metric: "accuracy",
      goal: "higher",
    },
  ],
  provenance: {
    canonicalName: "GPQA Diamond",
    sourceUrl: "https://arxiv.org/abs/2311.12022",
    supportStatus: "ready",
    comparabilityNotes:
      "Three deliberate divergences from the simple-evals reference protocol: " +
      '(1) prompt omits "Think step by step before answering." (inherited from openbench: faster, less leading); ' +
      "(2) per-record seeded option shuffle instead of one RNG stream (also fixes openbench's reseed-to-0 bug that pins the answer at B); " +
      "(3) 10 epochs (leaderboard series) vs n_repeats=4. " +
      "Scores are not comparable across the two protocol IDs.",
  },
};

export const TAU3_BENCH_BANKING_MANIFEST: EvalManifestV1 = {
  apiVersion: EVAL_MANIFEST_API_VERSION,
  id: TAU3_BENCH_BANKING_META.id,
  name: "τ³-bench Banking",
  version: "1",
  protocolId: "tau3-bench-banking.v1",
  dataset: { kind: "builtin", ref: "tau3-banking-dataset" },
  models: [
    { role: "candidate", overridable: true },
    {
      role: "user-simulator",
      defaultModel: TAU3_BENCH_BANKING_META.userModel,
      overridable: true,
    },
  ],
  solver: { kind: "builtin", ref: "tau3-banking-solver" },
  scorer: { kind: "builtin", ref: "tau3-banking-scorer" },
  sampling: { temperature: 0 },
  epochs: { default: TAU3_BENCH_BANKING_META.defaultEpochs, reducer: "mean" },
  execution: { tier: "trusted-local", drivers: ["process"] },
  reports: [
    {
      key: "score",
      label: "Task success",
      type: "scalar",
      metric: "success",
      goal: "higher",
    },
  ],
  provenance: {
    canonicalName: "tau3-bench banking",
    supportStatus: "ready",
    comparabilityNotes:
      "Dual-control conversations; user-simulator model is part of protocol identity. 5 epochs mirrors Artificial Analysis.",
  },
};

/** All first-party manifests declared so far, keyed by benchmark id. */
export const FIRST_PARTY_MANIFESTS: Readonly<Record<string, EvalManifestV1>> = {
  [GPQA_MANIFEST.id]: GPQA_MANIFEST,
  [TAU3_BENCH_BANKING_MANIFEST.id]: TAU3_BENCH_BANKING_MANIFEST,
};
