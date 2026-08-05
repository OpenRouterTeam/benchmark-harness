import type { Citation } from "../../harness/core";
import type { ValueOf } from "../../internal/guards";
import type { ZodShape } from "../../internal/zod";
import { z } from "../../internal/zod";

export const DEFAULT_BLOCKED_DOMAINS: readonly string[] = [
  "huggingface.co/datasets/perplexity-ai/draco",
  "r2cdn.perplexity.ai/pplx-draco.pdf",
  "arxiv.org/abs/2602.11685",
  "arxiv.org/pdf/2602.11685",
  "arxiv.org/html/2602.11685",
] as const;

export const CriterionSchema = z.object({
  id: z
    .string()
    .describe(
      "Unique id within the task, e.g. 'twfe-variance-weighted-decomposition'."
    ),
  section: z
    .string()
    .describe("Parent section title, e.g. 'Factual Accuracy'."),
  sectionId: z.string().describe("Parent section id, e.g. 'factual-accuracy'."),
  weight: z
    .number()
    .describe(
      "Scoring weight. Positive rewards meeting the criterion; negative penalises a violation."
    ),
  requirement: z
    .string()
    .describe("Human-readable description of what must be present/absent."),
});

export type Criterion = z.infer<typeof CriterionSchema>;

export const DracoTaskSchema = z.object({
  id: z.string().describe("UUID uniquely identifying this task."),
  problem: z
    .string()
    .describe("The complex research question/task to be answered."),
  domain: z
    .string()
    .describe(
      "One of 10 domain labels (e.g. 'Academic', 'Finance', 'Medicine')."
    ),
  criteria: z
    .array(CriterionSchema)
    .describe("Flat list of criteria extracted from the rubric JSON."),
  rawAnswer: z
    .record(z.string(), z.unknown())
    .describe("Original rubric JSON from the dataset, kept for reference."),
});

export type DracoTask = z.infer<typeof DracoTaskSchema>;

export const GenerationStatus = {
  Ok: "ok",
  Failed: "failed",
  Refused: "refused",
} as const;

export type GenerationStatus = ValueOf<typeof GenerationStatus>;

export const GenerationResultSchema = z.object({
  taskId: z.string(),
  experimentName: z.string(),
  status: z.enum([
    GenerationStatus.Ok,
    GenerationStatus.Failed,
    GenerationStatus.Refused,
  ]),
  content: z.string().nullable(),
  model: z.string(),
  generationId: z.string().nullable(),
  usage: z.record(z.string(), z.unknown()).nullish(),
  cost: z.number().nullish(),
  latencyMs: z.number().nullish(),
  error: z.string().nullish(),
  fusionAnalysis: z.record(z.string(), z.unknown()).nullish(),
  toolInvocations: z.array(z.record(z.string(), z.unknown())).default([]),
  contaminationSignals: z.array(z.string()).default([]),
  citations: z
    .array(
      z.object({
        url: z.string(),
        title: z.string(),
        startIndex: z.number(),
        endIndex: z.number(),
      } satisfies ZodShape<Citation>)
    )
    .default([]),
  timestamp: z.string(),
});

export type GenerationResult = z.infer<typeof GenerationResultSchema>;

export const VerdictValue = {
  Met: "MET",
  Unmet: "UNMET",
} as const;

export type VerdictValue = ValueOf<typeof VerdictValue>;

export const CriterionVerdictSchema = z.object({
  criterionId: z.string(),
  section: z.string(),
  weight: z.number(),
  verdict: z.enum([VerdictValue.Met, VerdictValue.Unmet]),
  justification: z.string(),
});

export type CriterionVerdict = z.infer<typeof CriterionVerdictSchema>;

export const JudgeRunStatus = {
  Ok: "ok",
  Failed: "failed",
  Skipped: "skipped",
} as const;

export type JudgeRunStatus = ValueOf<typeof JudgeRunStatus>;

export const JudgeRunSchema = z.object({
  runNum: z.number().int(),
  status: z.enum([
    JudgeRunStatus.Ok,
    JudgeRunStatus.Failed,
    JudgeRunStatus.Skipped,
  ]),
  verdicts: z.array(CriterionVerdictSchema).default([]),
  error: z.string().nullish(),
  judgeModel: z.string().default(""),
  cost: z.number().nullish(),
  latencyMs: z.number().nullish(),
  timestamp: z.string(),
});

export type JudgeRun = z.infer<typeof JudgeRunSchema>;

export const TaskVerdictsSchema = z.object({
  taskId: z.string(),
  experimentName: z.string(),
  judgeModel: z.string(),
  targetRuns: z.number().int(),
  runs: z.array(JudgeRunSchema).default([]),
});

export type TaskVerdicts = z.infer<typeof TaskVerdictsSchema>;

export const RunScoreSchema = z.object({
  runNum: z.number().int(),
  rawScore: z.number(),
  maxPossible: z.number(),
  normalizedScore: z
    .number()
    .describe("0–100, percentage of maximum achievable weighted score."),
  passRate: z
    .number()
    .describe("0–100, percentage of criteria on the favourable side."),
  criteriaMet: z.number().int(),
  criteriaTotal: z.number().int(),
});

export type RunScore = z.infer<typeof RunScoreSchema>;

export const TaskScoreSchema = z.object({
  taskId: z.string(),
  domain: z.string(),
  experimentName: z.string(),
  runScores: z.array(RunScoreSchema).default([]),
  meanNormalized: z.number().default(0),
  stdNormalized: z.number().default(0),
  meanPassRate: z.number().default(0),
  stdPassRate: z.number().default(0),
  judgeRunsCompleted: z.number().int().default(0),
  judgeRunsFailed: z.number().int().default(0),
  generationCost: z.number().nullish(),
  judgingCost: z.number().default(0),
  totalCost: z.number().default(0),
});

export type TaskScore = z.infer<typeof TaskScoreSchema>;

export const DracoToolType = {
  WebSearch: "openrouter:web_search",
  WebFetch: "openrouter:web_fetch",
  Shell: "openrouter:shell",
} as const;

export type DracoToolType = ValueOf<typeof DracoToolType>;

export const ShellEngine = {
  Auto: "auto",
  OpenRouter: "openrouter",
} as const;

export type ShellEngine = ValueOf<typeof ShellEngine>;

export const ShellEnvironmentSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("container_auto") }),
  z.object({
    type: z.literal("container_reference"),
    containerId: z.string().min(1).max(20),
  }),
]);

export type ShellEnvironment = z.infer<typeof ShellEnvironmentSchema>;

export const ShellToolParametersSchema = z.object({
  engine: z.enum([ShellEngine.Auto, ShellEngine.OpenRouter]).optional(),
  environment: ShellEnvironmentSchema.optional(),
  sleepAfterSeconds: z.number().int().positive().max(2592000).optional(),
});

export type ShellToolParameters = z.infer<typeof ShellToolParametersSchema>;

export const ToolEntrySchema = z.union([
  z.object({
    type: z.enum([DracoToolType.WebSearch, DracoToolType.WebFetch]),
    parameters: z.record(z.string(), z.unknown()).optional(),
  }),
  z.object({
    type: z.literal(DracoToolType.Shell),
    parameters: ShellToolParametersSchema.optional(),
  }),
]);

export type ToolEntry = z.infer<typeof ToolEntrySchema>;

export const DracoExperimentType = {
  Single: "single",
  Fusion: "fusion",
} as const;

export type DracoExperimentType = ValueOf<typeof DracoExperimentType>;

export const ProviderRoutingSchema = z.object({
  only: z.array(z.string()).optional(),
  ignore: z.array(z.string()).optional(),
});

export type ProviderRouting = z.infer<typeof ProviderRoutingSchema>;

export const JudgeReasoningEffort = [
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
] as const;

export type JudgeReasoningEffort = ValueOf<typeof JudgeReasoningEffort>;

export const DracoPanelConfigSchema = z.object({
  name: z.string(),
  description: z.string().default(""),
  type: z.enum([DracoExperimentType.Single, DracoExperimentType.Fusion]),
  model: z.string().optional(),
  fallbackModel: z.string().optional(),
  provider: ProviderRoutingSchema.optional(),
  synthesisModel: z.string().optional(),
  analysisModels: z.array(z.string()).max(4).default([]),
  tools: z.array(ToolEntrySchema).optional(),
  searchEngine: z
    .enum(["auto", "native", "exa", "parallel", "firecrawl"])
    .nullable()
    .default("exa"),
  blockedDomains: z.array(z.string()).default([...DEFAULT_BLOCKED_DOMAINS]),
  versionOverride: z.string().optional(),
  cacheNamespace: z.string().min(1).optional(),
  judgeModel: z.string().default("google/gemini-3.1-pro-preview"),
  judgeRuns: z.number().int().min(1).max(5).default(3),
  judgeTemperature: z.number().optional(),
  judgeReasoningEffort: z.enum(JudgeReasoningEffort).optional().default("low"),
  criterionConcurrency: z.number().int().min(1).max(40).default(10),
  timeout: z.number().int().default(1800),
  concurrency: z.number().int().min(1).max(20).default(5),
});

export type DracoPanelConfig = z.infer<typeof DracoPanelConfigSchema>;
