import { Tag } from "effect/Context";

import { z } from "../internal/zod";
import { ChatMessageSchema, ScoreValue } from "./core";

export const PersistedSampleOutcomeSchema = z.object({
  sampleScore: z.object({
    sampleId: z.string(),
    epoch: z.number(),
    score: z.object({
      value: z.enum([
        ScoreValue.Correct,
        ScoreValue.Incorrect,
        ScoreValue.Skipped,
      ]),
      answer: z.string().nullable(),
      explanation: z.string(),
    }),
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
  }),
  usage: z
    .object({
      inputTokens: z.number().optional(),
      outputTokens: z.number().optional(),
      totalTokens: z.number().optional(),
      reasoningTokens: z.number().optional(),
      totalCost: z.number().optional(),
      serverToolUse: z
        .object({
          webSearchRequests: z.number().optional(),
          toolCallsRequested: z.number().optional(),
          toolCallsExecuted: z.number().optional(),
        })
        .optional(),
    })
    .optional(),
  generationTimeMs: z.number().optional(),
});

export type PersistedSampleOutcome = z.infer<
  typeof PersistedSampleOutcomeSchema
>;

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
