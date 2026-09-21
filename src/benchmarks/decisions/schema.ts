import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";

export const DECISIONS_BENCHMARK_ID = "decisions" as const;

export const DECISIONS_CONTRACT_BENCHMARK_ID = "decisions_contract" as const;

export const DECISIONS_ROUTE = "/api/alpha/decisions" as const;

export const CHOICE_MAX_OPTIONS = 255;

export const SCORE_MIN_LEVELS = 2;

export const SCORE_MAX_LEVELS = 10;

export const PROBABILITY_SUM_TOLERANCE = 0.02;

export const SCORE_WEIGHTED_MEAN_TOLERANCE = 0.05;

const DecisionsTextSchema: z.ZodType<unknown> = z.union([
  z.string(),
  z.record(z.string(), z.unknown()),
  z.array(z.unknown()),
]);

export const NoulQuestionSchema = z.object({
  type: z.literal("noul"),
  instructions: DecisionsTextSchema,
  criteria: z
    .object({ true: DecisionsTextSchema, false: DecisionsTextSchema })
    .optional(),
});

export const ChoiceQuestionSchema = z.object({
  type: z.literal("choice"),
  instructions: DecisionsTextSchema,
  criteria: z.record(z.string(), DecisionsTextSchema.nullable()),
});

export const ScoreQuestionSchema = z.object({
  type: z.literal("score"),
  instructions: DecisionsTextSchema,
  criteria: z.array(DecisionsTextSchema),
});

export const DecisionsQuestionSchema = z.discriminatedUnion("type", [
  NoulQuestionSchema,
  ChoiceQuestionSchema,
  ScoreQuestionSchema,
]);

export type NoulQuestion = z.infer<typeof NoulQuestionSchema>;

export type ChoiceQuestion = z.infer<typeof ChoiceQuestionSchema>;

export type ScoreQuestion = z.infer<typeof ScoreQuestionSchema>;

export type DecisionsQuestion = z.infer<typeof DecisionsQuestionSchema>;

export const DecisionsProviderPreferencesSchema = z.object({
  only: z.array(z.string()).optional(),
  ignore: z.array(z.string()).optional(),
  allow_fallbacks: z.boolean().optional(),
});

export const DecisionsRequestSchema = z.object({
  model: z.string(),
  state: z.unknown(),
  questions: z.record(z.string(), DecisionsQuestionSchema),
  provider: DecisionsProviderPreferencesSchema.optional(),
});

export type DecisionsRequest = z.infer<typeof DecisionsRequestSchema>;

const ProbabilitiesSchema = z.record(z.string(), z.number());

export const NoulAnswerSchema = z
  .object({
    type: z.literal("noul"),
    noul: z.number(),
  })
  .passthrough();

export const ChoiceAnswerSchema = z
  .object({
    type: z.literal("choice"),
    choice: z.string(),
    probabilities: ProbabilitiesSchema,
    confidence: z.number().optional(),
  })
  .passthrough();

export const ScoreAnswerSchema = z
  .object({
    type: z.literal("score"),
    score: z.number(),
    legend: z.record(z.string(), z.unknown()),
    probabilities: ProbabilitiesSchema,
    confidence: z.number().optional(),
  })
  .passthrough();

export const DecisionsAnswerSchema = z.discriminatedUnion("type", [
  NoulAnswerSchema,
  ChoiceAnswerSchema,
  ScoreAnswerSchema,
]);

export type DecisionsAnswer = z.infer<typeof DecisionsAnswerSchema>;

export const DecisionsUsageSchema = z
  .object({
    input_tokens: z.number(),
    output_tokens: z.number(),
    cost: z.number().optional(),
  })
  .passthrough();

export const DecisionsResponseSchema = z
  .object({
    id: z.string().optional(),
    model: z.string(),
    provider: z.string().optional(),
    answers: z.record(z.string(), DecisionsAnswerSchema),
    usage: DecisionsUsageSchema.optional(),
  })
  .passthrough();

export type DecisionsResponse = z.infer<typeof DecisionsResponseSchema>;

export const DecisionArm = {
  Decision: "decision",
  Llm: "llm",
} as const;

export type DecisionArm = ValueOf<typeof DecisionArm>;

export const DECISION_ARMS = [DecisionArm.Decision, DecisionArm.Llm] as const;

export const DecisionTaskId = {
  Banking77: "banking77",
  MassiveIntent: "massive_intent",
  Dbpedia14: "dbpedia_14",
  BoolQ: "boolq",
  PromptInjection: "prompt_injection",
  Sst5: "sst5",
  MmluPro: "mmlu_pro",
  Xnli: "xnli",
} as const;

export type DecisionTaskId = ValueOf<typeof DecisionTaskId>;

export const DECISION_TASK_IDS = [
  DecisionTaskId.Banking77,
  DecisionTaskId.MassiveIntent,
  DecisionTaskId.Dbpedia14,
  DecisionTaskId.BoolQ,
  DecisionTaskId.PromptInjection,
  DecisionTaskId.Sst5,
  DecisionTaskId.MmluPro,
  DecisionTaskId.Xnli,
] as const;

export const DecisionVariant = {
  Base: "base",
  Shuffled: "shuffled",
  Reversed: "reversed",
  Distractor: "distractor",
  NoCriteria: "no_criteria",
  StateObject: "state_object",
  EvidenceRemoved: "evidence_removed",
} as const;

export type DecisionVariant = ValueOf<typeof DecisionVariant>;

export const DECISION_VARIANTS = [
  DecisionVariant.Base,
  DecisionVariant.Shuffled,
  DecisionVariant.Reversed,
  DecisionVariant.Distractor,
  DecisionVariant.NoCriteria,
  DecisionVariant.StateObject,
  DecisionVariant.EvidenceRemoved,
] as const;

export const DecisionTaskSpecSchema = z.object({
  task: z.enum(DECISION_TASK_IDS),
  variant: z.enum(DECISION_VARIANTS),
  language: z.string(),
  questionId: z.string(),
  state: z.unknown(),
  question: DecisionsQuestionSchema,
  labels: z.array(z.string()).min(1),
  gold: z.string(),
  goldKnowable: z.boolean(),
  stateText: z.string(),
});

export type DecisionTaskSpec = z.infer<typeof DecisionTaskSpecSchema>;

export const DecisionOutcomeSchema = z.object({
  arm: z.enum(DECISION_ARMS),
  model: z.string(),
  provider: z.string().optional(),
  distribution: z.record(z.string(), z.number()),
  argmax: z.string().nullable(),
  confidence: z.number().nullable(),
  scoreValue: z.number().nullable(),
  latencyMs: z.number(),
  cost: z.number().nullable(),
  inputTokens: z.number().nullable(),
  outputTokens: z.number().nullable(),
  violations: z.array(z.string()),
  rawAnswer: z.unknown(),
});

export type DecisionOutcome = z.infer<typeof DecisionOutcomeSchema>;

export const DECISION_OUTCOME_METADATA_KEY = "decision" as const;

export const DECISION_SPEC_METADATA_KEY = "decisionSpec" as const;
