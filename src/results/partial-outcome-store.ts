import { z } from "zod";

import { ChatMessageSchema, ScoreValue } from "../harness/core";
import type { SampleOutcome } from "../harness/run";

export interface PartialOutcomeStoreService {
  readonly read: () => Promise<readonly SampleOutcome[] | null>;
  readonly write: (outcomes: readonly SampleOutcome[]) => Promise<void>;
  readonly remove: () => Promise<void>;
}

const ScoreValueSchema = z.enum([
  ScoreValue.Correct,
  ScoreValue.Incorrect,
  ScoreValue.Skipped,
]);

const ScorerTrajectorySchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("verifier_log"), log: z.string() }),
  z.object({
    kind: z.literal("judge_runs"),
    runs: z.array(z.unknown()).readonly(),
  }),
]);

const ScoreSchema = z.object({
  value: ScoreValueSchema,
  answer: z.string().nullable(),
  explanation: z.string(),
  trajectory: ScorerTrajectorySchema.optional(),
});

const SampleScoreSchema = z.object({
  sampleId: z.string(),
  epoch: z.number(),
  score: ScoreSchema,
  messages: z.array(ChatMessageSchema).readonly().optional(),
  responseItems: z
    .array(z.record(z.string(), z.unknown()).readonly())
    .readonly()
    .optional(),
  requestBody: z.record(z.string(), z.unknown()).readonly().optional(),
  generationIds: z.array(z.string()).readonly().optional(),
  metadata: z.record(z.string(), z.unknown()).readonly().optional(),
  input: z.string().optional(),
  target: z.string().optional(),
});

const ServerToolUseCountsSchema = z.object({
  webSearchRequests: z.number().optional(),
  toolCallsRequested: z.number().optional(),
  toolCallsExecuted: z.number().optional(),
});

const ModelUsageSchema = z.object({
  inputTokens: z.number().optional(),
  outputTokens: z.number().optional(),
  totalTokens: z.number().optional(),
  reasoningTokens: z.number().optional(),
  totalCost: z.number().optional(),
  serverToolUse: ServerToolUseCountsSchema.optional(),
});

export const SampleOutcomeSchema = z.object({
  sampleScore: SampleScoreSchema,
  usage: ModelUsageSchema.optional(),
  generationTimeMs: z.number().optional(),
});

export const PartialOutcomesSchema = z.array(SampleOutcomeSchema).readonly();
