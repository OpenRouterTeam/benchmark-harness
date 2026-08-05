import { Tag } from "effect/Context";
import { TaggedError } from "effect/Data";

import { z } from "../internal/zod";
import { ScorerTrajectorySchema } from "../results/parquet-schema";
import type { ModelUsage, Score } from "./core";
import { ChatMessageSchema, ScoreValue } from "./core";
import type { SampleScore } from "./metric";

/**
 * Durable per-(sample, epoch) result records, persisted after each successful
 * eval so a retried chunk can skip already-completed work. Distinct from
 * `CheckpointStore` (in-flight sandbox state) and `ResultStore` (whole-chunk
 * parquet): a record here is a finished outcome that survives activity
 * retries. Records are idempotent — rewriting the same (sample, epoch) key
 * produces the same content, so concurrent duplicate writes are harmless.
 */

export const SAMPLE_RESULT_FORMAT_VERSION = 1;

/** A sample-result store read or write failed. */
// oxlint-disable-next-line unicorn/throw-new-error -- 1.74 false positive on Effect TaggedError class declaration
export class SampleResultStoreError extends TaggedError(
  "SampleResultStoreError"
)<{
  readonly message: string;
}> {}

const ScoreSchema = z.object({
  value: z.enum([ScoreValue.Correct, ScoreValue.Incorrect, ScoreValue.Skipped]),
  answer: z.string().nullable(),
  explanation: z.string(),
  trajectory: ScorerTrajectorySchema.optional(),
});

/**
 * The sample-score payload of a record: everything beyond `sample_id`/`epoch`
 * (stored top-level) needed to reconstruct a {@link SampleScore}. Nested
 * payloads (messages, response items) keep their canonical in-memory shapes.
 */
const SampleScorePayloadSchema = z.object({
  score: ScoreSchema,
  messages: z.array(ChatMessageSchema).readonly().optional(),
  response_items: z
    .array(z.record(z.string(), z.unknown()).readonly())
    .readonly()
    .optional(),
  generation_ids: z.array(z.string()).readonly().optional(),
  metadata: z.record(z.string(), z.unknown()).optional(),
  input: z.string().optional(),
  target: z.string().optional(),
});

const SampleUsageSchema = z.object({
  input_tokens: z.number(),
  output_tokens: z.number(),
  total_tokens: z.number(),
  reasoning_tokens: z.number(),
  total_cost: z.number(),
});

/**
 * Pinned wire contract for one completed (sample, epoch) record. Downstream
 * consumers (aggregate derivation, ingestion pipelines) are specced against
 * this shape — do not change field names or the object path scheme
 * (`samples/<parentWorkflowId>/<benchmarkId>/<safeModel>/<sampleIndex>-<epoch>.json`)
 * without flagging it.
 *
 * `degraded: true` marks a score synthesized from a model/solver
 * infrastructure failure, written only at the end of a chunk's run. Degraded
 * records must be ignored by the retry skip-list (the sample is re-run for
 * free) but counted by the finalization fold per their score value —
 * Skipped into skipped_questions, Incorrect into the accuracy denominator.
 */
export const SampleResultRecordSchema = z.object({
  format_version: z.literal(SAMPLE_RESULT_FORMAT_VERSION),
  parent_workflow_id: z.string(),
  child_workflow_id: z.string(),
  chunk_index: z.number().nullable(),
  benchmark_id: z.string(),
  model: z.string(),
  sample_id: z.string(),
  /** Absolute dataset index (not chunk-relative), unique per run regardless of chunking. */
  sample_index: z.number(),
  epoch: z.number(),
  sample_score: SampleScorePayloadSchema,
  usage: SampleUsageSchema.nullable(),
  generation_time_ms: z.number().nullable(),
  /** Score synthesized from an infrastructure failure; absent on genuine evaluations. */
  degraded: z.literal(true).optional(),
  created_at: z.string(),
});

export type SampleResultRecord = z.infer<typeof SampleResultRecordSchema>;

/** One completed (sample, epoch) outcome, in harness in-memory shape. */
export interface CompletedSampleEntry {
  /** Absolute dataset index (not chunk-relative). */
  readonly sampleIndex: number;
  readonly epoch: number;
  readonly sampleScore: SampleScore;
  readonly usage?: ModelUsage;
  readonly generationTimeMs?: number;
  /** Score synthesized from an infrastructure failure; never skip-seeded on retry. */
  readonly degraded?: true;
}

export interface SampleResultStoreService {
  /** Persist one completed (sample, epoch) result. Idempotent per key. */
  readonly write: (entry: CompletedSampleEntry) => Promise<void>;
  /** Existing completed entries whose sampleIndex falls in the half-open `[start, end)` range. */
  readonly list: (range: {
    readonly start?: number;
    readonly end?: number;
  }) => Promise<readonly CompletedSampleEntry[]>;
}

export class SampleResultStore extends Tag(
  "@openrouter/bench-harness/sample-result-store"
)<SampleResultStore, SampleResultStoreService>() {}

export const NOOP_SAMPLE_RESULT_STORE: SampleResultStoreService = {
  write: async () => {},
  list: async () => [],
};

export interface SampleResultEnvelope {
  readonly parentWorkflowId: string;
  readonly childWorkflowId: string;
  readonly chunkIndex: number | null;
  readonly benchmarkId: string;
  readonly model: string;
}

export function encodeSampleResultRecord(
  envelope: SampleResultEnvelope,
  entry: CompletedSampleEntry
): SampleResultRecord {
  const { sampleScore, usage } = entry;
  return {
    format_version: SAMPLE_RESULT_FORMAT_VERSION,
    parent_workflow_id: envelope.parentWorkflowId,
    child_workflow_id: envelope.childWorkflowId,
    chunk_index: envelope.chunkIndex,
    benchmark_id: envelope.benchmarkId,
    model: envelope.model,
    sample_id: sampleScore.sampleId,
    sample_index: entry.sampleIndex,
    epoch: entry.epoch,
    sample_score: encodeSampleScorePayload(sampleScore),
    usage: usage === undefined ? null : encodeUsage(usage),
    generation_time_ms: entry.generationTimeMs ?? null,
    ...(entry.degraded === true && { degraded: true }),
    created_at: new Date().toISOString(),
  };
}

/**
 * Same rest-guard as {@link encodeSampleScorePayload}, for `ModelUsage`
 * fields. `serverToolUse` is deliberately not part of the pinned v1 record
 * contract — the resume aggregate only folds token/cost totals — so it is
 * consumed here explicitly rather than silently dropped.
 */
function encodeUsage(usage: ModelUsage): SampleResultRecord["usage"] {
  const {
    inputTokens,
    outputTokens,
    totalTokens,
    reasoningTokens,
    totalCost,
    serverToolUse: _serverToolUse,
    ...rest
  } = usage;
  rest satisfies Record<never, never>;
  return {
    input_tokens: inputTokens ?? 0,
    output_tokens: outputTokens ?? 0,
    total_tokens: totalTokens ?? 0,
    reasoning_tokens: reasoningTokens ?? 0,
    total_cost: totalCost ?? 0,
  };
}

/**
 * Rename the `SampleScore` fields a record carries beyond `sample_id` /
 * `epoch`. Destructuring the whole score keeps the payload honest: adding a
 * field to `SampleScore` without persisting it fails to compile on `rest`
 * instead of silently dropping data.
 */
function encodeSampleScorePayload(
  sampleScore: SampleScore
): SampleResultRecord["sample_score"] {
  const {
    sampleId: _sampleId,
    epoch: _epoch,
    score,
    messages,
    responseItems,
    generationIds,
    metadata,
    input,
    target,
    ...rest
  } = sampleScore;
  rest satisfies Record<never, never>;
  return {
    score: encodeScore(score),
    ...(messages !== undefined && { messages }),
    ...(responseItems !== undefined && { response_items: responseItems }),
    ...(generationIds !== undefined && { generation_ids: generationIds }),
    ...(metadata !== undefined && { metadata }),
    ...(input !== undefined && { input }),
    ...(target !== undefined && { target }),
  };
}

/** Same rest-guard as {@link encodeSampleScorePayload}, for `Score` fields. */
function encodeScore(
  score: Score
): SampleResultRecord["sample_score"]["score"] {
  const { value, answer, explanation, trajectory, ...rest } = score;
  rest satisfies Record<never, never>;
  return {
    value,
    answer,
    explanation,
    ...(trajectory !== undefined && { trajectory }),
  };
}

export function decodeSampleResultEntry(
  record: SampleResultRecord
): CompletedSampleEntry {
  const payload = record.sample_score;
  const sampleScore: SampleScore = {
    sampleId: record.sample_id,
    epoch: record.epoch,
    score: payload.score,
    ...(payload.messages !== undefined && { messages: payload.messages }),
    ...(payload.response_items !== undefined && {
      responseItems: payload.response_items,
    }),
    ...(payload.generation_ids !== undefined && {
      generationIds: payload.generation_ids,
    }),
    ...(payload.metadata !== undefined && { metadata: payload.metadata }),
    ...(payload.input !== undefined && { input: payload.input }),
    ...(payload.target !== undefined && { target: payload.target }),
  };
  return {
    sampleIndex: record.sample_index,
    epoch: record.epoch,
    sampleScore,
    ...(record.usage !== null && {
      usage: {
        inputTokens: record.usage.input_tokens,
        outputTokens: record.usage.output_tokens,
        totalTokens: record.usage.total_tokens,
        reasoningTokens: record.usage.reasoning_tokens,
        totalCost: record.usage.total_cost,
      },
    }),
    ...(record.generation_time_ms !== null && {
      generationTimeMs: record.generation_time_ms,
    }),
    ...(record.degraded === true && { degraded: true }),
  };
}
