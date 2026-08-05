/**
 * EvalSpec v1: a fully declarative eval definition — dataset + prompt +
 * deterministic scorer. User evals are **data, not code**, so they run on the
 * existing worker fleet with no isolation concerns. This is the rung-1 surface
 * of the evals product plan (docs/plans/2026-07-28-001-feat-evals-product-
 * staging-plan.md); user-TypeScript evals are a later rung.
 *
 * Workflow-safe: schema-only module, no heavy imports.
 */
import { z } from "../../internal/zod";

/** One inline case: a prompt and its grading target. */
export const InlineCaseSchema = z.object({
  id: z.string().optional(),
  input: z.string(),
  target: z.string(),
  metadata: z.record(z.string(), z.unknown()).optional(),
});
export type InlineCase = z.infer<typeof InlineCaseSchema>;

export const InlineDatasetSchema = z.object({
  kind: z.literal("inline"),
  cases: z.array(InlineCaseSchema).min(1).max(10_000),
});

export const HfDatasetSpecSchema = z.object({
  kind: z.literal("hf"),
  /** Dataset repo id, e.g. "openai/gsm8k". */
  dataset: z
    .string()
    .regex(
      /^[\w.-]+\/[\w.-]+$/,
      "dataset must be a HuggingFace repo id (owner/name); no slashes beyond the separator"
    ),
  config: z.string().default("default"),
  split: z.string().default("test"),
  /** Record field used as the prompt input. */
  inputField: z.string(),
  /** Record field used as the grading target. */
  targetField: z.string(),
  /**
   * Pinned dataset git revision (commit SHA). When set, the run fails closed
   * if the upstream default branch has moved — an upstream dataset change is
   * a new comparability series, never a silent score shift.
   */
  revision: z.string().optional(),
});

export const EvalDatasetSchema = z.discriminatedUnion("kind", [
  InlineDatasetSchema,
  HfDatasetSpecSchema,
]);
export type EvalDataset = z.infer<typeof EvalDatasetSchema>;

/**
 * Deterministic scorers only, on purpose: every verdict is reproducible from
 * the stored trajectory. An LLM-judge scorer is a deliberate follow-up — it
 * needs judge pinning and `judge-dependent` labeling on results before
 * customer exposure.
 */
export const EvalScorerSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("exact"),
    caseSensitive: z.boolean().default(false),
    trim: z.boolean().default(true),
  }),
  z.object({
    kind: z.literal("contains"),
    caseSensitive: z.boolean().default(false),
  }),
  z.object({
    kind: z.literal("regex"),
    /** Correct when the completion matches. Compiled with the `u` flag. */
    pattern: z.string().refine(
      (value) => {
        try {
          void new RegExp(value, "u");
          return true;
        } catch {
          return false;
        }
      },
      {
        message:
          "pattern must be a valid regular expression (compiled with the u flag)",
      }
    ),
    caseSensitive: z.boolean().default(false),
  }),
  z.object({
    /** MCQ letter extraction (shared with gpqa/mmlu): target is a letter A-Z. */
    kind: z.literal("choice"),
  }),
  z.object({
    /** Compares the last number in the completion to the numeric target. */
    kind: z.literal("numeric"),
    absoluteTolerance: z.number().nonnegative().default(0),
    relativeTolerance: z.number().nonnegative().default(0),
  }),
]);
export type EvalScorer = z.infer<typeof EvalScorerSchema>;

export const EvalSpecSchema = z.object({
  specVersion: z.literal(1).default(1),
  /** Display name; not part of scoring identity. */
  name: z.string().min(1).max(200),
  dataset: EvalDatasetSchema,
  /**
   * Prompt template applied to each case's input. `{input}` is replaced with
   * the case input; omitted → the raw input is the user message.
   */
  promptTemplate: z.string().optional(),
  /** Optional system message prepended to every conversation. */
  systemPrompt: z.string().optional(),
  scorer: EvalScorerSchema,
});
export type EvalSpec = z.infer<typeof EvalSpecSchema>;
