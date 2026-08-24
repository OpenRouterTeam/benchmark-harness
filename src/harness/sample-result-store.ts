import { Tag } from "effect/Context";

import type { ZodShape } from "../internal/zod";
import { z } from "../internal/zod";
import type {
  ModelUsage,
  ResponseItem,
  Score,
  ScorerTrajectory,
  ServerToolUseCounts,
} from "./core";
import { ChatMessageSchema, ScoreValue } from "./core";
import type { SampleScore } from "./metric";

export interface PersistedSampleOutcome {
  readonly sampleScore: SampleScore;
  readonly usage?: ModelUsage;
  readonly generationTimeMs?: number;
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

export const PersistedSampleOutcomeSchema = z.object({
  sampleScore: SampleScoreSchema,
  usage: ModelUsageSchema.optional(),
  generationTimeMs: z.number().optional(),
} satisfies ZodShape<PersistedSampleOutcome>);

export function sampleResultKey(sampleId: string, epoch: number): string {
  return `${sampleId}/${epoch}`;
}

export interface SampleResultStoreService {
  readonly read: (key: string) => Promise<PersistedSampleOutcome | null>;
  readonly write: (key: string, data: PersistedSampleOutcome) => Promise<void>;
}

export class SampleResultStore extends Tag(
  "@openrouter/bench-harness/sample-result-store"
)<SampleResultStore, SampleResultStoreService>() {}

export const NOOP_SAMPLE_RESULT_STORE: SampleResultStoreService = {
  read: async () => null,
  write: async () => {},
};
