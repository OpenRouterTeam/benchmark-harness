import { ChatMessageSchema, ScoreValue } from "../harness/core";
import type {
  ModelUsage,
  ResponseItem,
  Score,
  ScorerTrajectory,
  ServerToolUseCounts,
} from "../harness/core";
import type { SampleScore } from "../harness/metric";
import type { SampleOutcome } from "../harness/run";
import type { ZodShape } from "../internal/zod";
import { z } from "../internal/zod";

export interface PartialOutcomeRunScope {
  readonly epochs: number;
  readonly range?: {
    readonly start?: number;
    readonly end?: number;
  };
}

export interface PartialOutcomesPayload {
  readonly scope: PartialOutcomeRunScope;
  readonly outcomes: readonly SampleOutcome[];
}

export interface PartialOutcomeStoreService {
  readonly read: () => Promise<PartialOutcomesPayload | null>;
  readonly write: (payload: PartialOutcomesPayload) => Promise<void>;
  readonly remove: () => Promise<void>;
}

const ScoreValueSchema = z.enum([
  ScoreValue.Correct,
  ScoreValue.Incorrect,
  ScoreValue.Skipped,
]);

type VerifierLogTrajectory = Extract<
  ScorerTrajectory,
  { kind: "verifier_log" }
>;
type JudgeRunsTrajectory = Extract<ScorerTrajectory, { kind: "judge_runs" }>;

const ScorerTrajectorySchema: z.ZodType<ScorerTrajectory> =
  z.discriminatedUnion("kind", [
    z.object({
      kind: z.literal("verifier_log"),
      log: z.string(),
    } satisfies ZodShape<VerifierLogTrajectory>),
    z.object({
      kind: z.literal("judge_runs"),
      runs: z.array(z.unknown()).readonly(),
    } satisfies ZodShape<JudgeRunsTrajectory>),
  ]);

const ScoreSchema = z.object({
  value: ScoreValueSchema,
  answer: z.string().nullable(),
  explanation: z.string(),
  trajectory: ScorerTrajectorySchema.optional(),
} satisfies ZodShape<Score>);

const ResponseItemSchema: z.ZodType<ResponseItem> = z
  .record(z.string(), z.unknown())
  .readonly();

const SampleScoreSchema = z.object({
  sampleId: z.string(),
  epoch: z.number(),
  score: ScoreSchema,
  messages: z.array(ChatMessageSchema).readonly().optional(),
  responseItems: z.array(ResponseItemSchema).readonly().optional(),
  requestBody: z.record(z.string(), z.unknown()).readonly().optional(),
  generationIds: z.array(z.string()).readonly().optional(),
  metadata: z.record(z.string(), z.unknown()).readonly().optional(),
  input: z.string().optional(),
  target: z.string().optional(),
} satisfies ZodShape<SampleScore>);

const ServerToolUseCountsSchema = z.object({
  webSearchRequests: z.number().optional(),
  toolCallsRequested: z.number().optional(),
  toolCallsExecuted: z.number().optional(),
} satisfies ZodShape<ServerToolUseCounts>);

const ModelUsageSchema = z.object({
  inputTokens: z.number().optional(),
  outputTokens: z.number().optional(),
  totalTokens: z.number().optional(),
  reasoningTokens: z.number().optional(),
  totalCost: z.number().optional(),
  serverToolUse: ServerToolUseCountsSchema.optional(),
} satisfies ZodShape<ModelUsage>);

export const SampleOutcomeSchema = z.object({
  sampleScore: SampleScoreSchema,
  usage: ModelUsageSchema.optional(),
  generationTimeMs: z.number().optional(),
} satisfies ZodShape<SampleOutcome>);

const PartialOutcomeRunScopeRangeSchema = z.object({
  start: z.number().optional(),
  end: z.number().optional(),
} satisfies ZodShape<NonNullable<PartialOutcomeRunScope["range"]>>);

export const PartialOutcomeRunScopeSchema = z.object({
  epochs: z.number(),
  range: PartialOutcomeRunScopeRangeSchema.optional(),
} satisfies ZodShape<PartialOutcomeRunScope>);

export const PartialOutcomesPayloadSchema = z.object({
  scope: PartialOutcomeRunScopeSchema,
  outcomes: z.array(SampleOutcomeSchema).readonly(),
} satisfies ZodShape<PartialOutcomesPayload>);

export function isSameRunScope(
  a: PartialOutcomeRunScope,
  b: PartialOutcomeRunScope
): boolean {
  return (
    a.epochs === b.epochs &&
    a.range?.start === b.range?.start &&
    a.range?.end === b.range?.end
  );
}
