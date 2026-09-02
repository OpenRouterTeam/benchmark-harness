import { unsafeNow, formatIso } from "effect/DateTime";
import type { AsyncBuffer } from "hyparquet";
import { parquetReadObjects } from "hyparquet";
import { parquetWriteBuffer } from "hyparquet-writer";

import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import type { BenchmarkPrimaryScore } from "../benchmarks/types";
import type {
  ModelMessage,
  ContentPart,
  ToolCall,
  UsageTotals,
} from "../harness/core";
import { ScoreValue } from "../harness/core";
import type { AggregateMetrics, SampleScore } from "../harness/metric";
import { aggregateScores } from "../harness/metric";
import type { RunResult } from "../harness/run";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import { firstZodIssueMessage, parseSchema, z } from "../internal/zod";
import type { BenchmarkResultRow } from "./parquet-schema";
import {
  RESULT_FORMAT_VERSION,
  RESULT_WRITER,
  BenchmarkResultRowSchema,
} from "./parquet-schema";

export interface ExtraScore {
  readonly name: string;
  readonly metrics: Readonly<
    Record<
      string,
      {
        readonly value: number;
      }
    >
  >;
}

export interface ParquetRunMeta {
  readonly task: string;
  readonly model: string;
  readonly epochs: number;
  readonly temperature?: number;
  readonly createdAt?: string;
  readonly benchmarkConfig?: BenchmarkRunConfig;
}

export interface RunResultToParquetInput {
  readonly result: RunResult;
  readonly meta: ParquetRunMeta;
  readonly extraScores?: readonly ExtraScore[];
  readonly primaryScore?: BenchmarkPrimaryScore;
}

interface ColumnSpec {
  readonly name: string;
  readonly type:
    | "INT32"
    | "INT64"
    | "FLOAT"
    | "DOUBLE"
    | "BOOLEAN"
    | "STRING"
    | "JSON";
  readonly nullable: boolean;
}

const COLUMN_SPECS = [
  { name: "format_version", type: "INT32", nullable: false },
  { name: "task", type: "STRING", nullable: false },
  { name: "model", type: "STRING", nullable: false },
  { name: "epochs", type: "INT32", nullable: false },
  { name: "temperature", type: "FLOAT", nullable: true },
  { name: "benchmark_config", type: "JSON", nullable: true },
  { name: "created_at", type: "STRING", nullable: false },
  { name: "accuracy", type: "DOUBLE", nullable: false },
  { name: "total_questions", type: "INT32", nullable: false },
  { name: "correct_answers", type: "INT32", nullable: false },
  { name: "input_tokens", type: "INT32", nullable: false },
  { name: "output_tokens", type: "INT32", nullable: false },
  { name: "total_tokens", type: "INT32", nullable: false },
  { name: "reasoning_tokens", type: "INT32", nullable: false },
  { name: "total_cost", type: "DOUBLE", nullable: false },
  { name: "generation_time_ms", type: "INT32", nullable: false },
  { name: "epoch_total_questions", type: "INT32", nullable: true },
  { name: "epoch_correct_answers", type: "INT32", nullable: true },
  { name: "extra_scores", type: "JSON", nullable: true },
  { name: "primary_score", type: "JSON", nullable: true },
  { name: "sample_id", type: "STRING", nullable: false },
  { name: "epoch", type: "INT32", nullable: false },
  { name: "input", type: "STRING", nullable: true },
  { name: "target", type: "STRING", nullable: true },
  { name: "score_value", type: "STRING", nullable: false },
  { name: "answer", type: "STRING", nullable: true },
  { name: "explanation", type: "STRING", nullable: true },
  { name: "scorer_trajectory", type: "JSON", nullable: true },
  { name: "response_items", type: "JSON", nullable: true },
  { name: "request_body", type: "JSON", nullable: true },
  { name: "generation_ids", type: "JSON", nullable: true },
  { name: "messages", type: "JSON", nullable: true },
  { name: "metadata", type: "JSON", nullable: true },
] as const satisfies readonly ColumnSpec[];

type ColumnName = (typeof COLUMN_SPECS)[number]["name"];

export function runResultToParquet(input: RunResultToParquetInput): Buffer {
  const { result, meta, extraScores, primaryScore } = input;
  const { metrics, usage, sampleScores } = result;
  const createdAt = meta.createdAt ?? formatIso(unsafeNow());
  const extraScoresJson =
    extraScores !== undefined && extraScores.length > 0
      ? JSON.stringify(extraScores)
      : null;
  const primaryScoreJson =
    primaryScore !== undefined ? JSON.stringify(primaryScore) : null;
  const benchmarkConfigJson =
    meta.benchmarkConfig !== undefined
      ? JSON.stringify(meta.benchmarkConfig)
      : null;
  const rowCtx: RowContext = {
    metrics,
    usage,
    epochTotalQuestions: sampleScores.filter(
      (s) => s.score.value !== ScoreValue.Skipped
    ).length,
    epochCorrectAnswers: sampleScores.filter(
      (s) => s.score.value === ScoreValue.Correct
    ).length,
    meta,
    createdAt,
    extraScoresJson,
    primaryScoreJson,
    benchmarkConfigJson,
  };
  const columnData = COLUMN_SPECS.map((spec) => ({
    name: spec.name,
    type: spec.type,
    nullable: spec.nullable,
    data: sampleScores.map((s) => cellValue(spec.name, rowCtx, s)),
  }));
  const arrayBuffer = parquetWriteBuffer({
    columnData,
    codec: "SNAPPY",
    kvMetadata: [
      { key: "writer", value: RESULT_WRITER },
      { key: "schema_version", value: String(RESULT_FORMAT_VERSION) },
      { key: "task", value: meta.task },
      { key: "model", value: meta.model },
      { key: "created_at", value: createdAt },
    ],
  });
  return Buffer.from(arrayBuffer);
}

function cellValue(name: ColumnName, ctx: RowContext, s: SampleScore): unknown {
  switch (name) {
    case "format_version": {
      return RESULT_FORMAT_VERSION;
    }
    case "task": {
      return ctx.meta.task;
    }
    case "model": {
      return ctx.meta.model;
    }
    case "epochs": {
      return ctx.meta.epochs;
    }
    case "temperature": {
      return ctx.meta.temperature ?? null;
    }
    case "benchmark_config": {
      return ctx.benchmarkConfigJson;
    }
    case "created_at": {
      return ctx.createdAt;
    }
    case "accuracy": {
      return ctx.metrics.accuracy;
    }
    case "total_questions": {
      return ctx.metrics.totalQuestions;
    }
    case "correct_answers": {
      return ctx.metrics.correctAnswers;
    }
    case "input_tokens": {
      return ctx.usage.inputTokens;
    }
    case "output_tokens": {
      return ctx.usage.outputTokens;
    }
    case "total_tokens": {
      return ctx.usage.totalTokens;
    }
    case "reasoning_tokens": {
      return ctx.usage.reasoningTokens;
    }
    case "total_cost": {
      return ctx.usage.totalCost;
    }
    case "generation_time_ms": {
      return ctx.usage.generationTimeMs;
    }
    case "epoch_total_questions": {
      return ctx.epochTotalQuestions;
    }
    case "epoch_correct_answers": {
      return ctx.epochCorrectAnswers;
    }
    case "extra_scores": {
      return ctx.extraScoresJson;
    }
    case "primary_score": {
      return ctx.primaryScoreJson;
    }
    case "sample_id": {
      return s.sampleId;
    }
    case "epoch": {
      return s.epoch;
    }
    case "input": {
      return s.input ?? null;
    }
    case "target": {
      return s.target ?? null;
    }
    case "score_value": {
      return s.score.value;
    }
    case "answer": {
      return s.score.answer;
    }
    case "explanation": {
      return s.score.explanation || null;
    }
    case "scorer_trajectory": {
      return s.score.trajectory !== undefined
        ? JSON.stringify(s.score.trajectory)
        : null;
    }
    case "response_items": {
      return s.responseItems !== undefined && s.responseItems.length > 0
        ? JSON.stringify(s.responseItems)
        : null;
    }
    case "request_body": {
      return s.requestBody !== undefined ? JSON.stringify(s.requestBody) : null;
    }
    case "generation_ids": {
      return s.generationIds !== undefined && s.generationIds.length > 0
        ? JSON.stringify(s.generationIds)
        : null;
    }
    case "messages": {
      return s.messages !== undefined && s.messages.length > 0
        ? JSON.stringify(s.messages.map(messageToPojo))
        : null;
    }
    case "metadata": {
      return s.metadata !== undefined ? JSON.stringify(s.metadata) : null;
    }
    default: {
      name satisfies never;
      throw new Error(`Unhandled column: ${name}`);
    }
  }
}

interface RowContext {
  readonly metrics: AggregateMetrics;
  readonly usage: UsageTotals;
  readonly epochTotalQuestions: number;
  readonly epochCorrectAnswers: number;
  readonly meta: ParquetRunMeta;
  readonly createdAt: string;
  readonly extraScoresJson: string | null;
  readonly primaryScoreJson: string | null;
  readonly benchmarkConfigJson: string | null;
}

function messageToPojo(msg: ModelMessage): Record<string, unknown> {
  const pojo: Record<string, unknown> = {
    role: msg.role,
    content: msg.content,
  };
  if (msg.reasoning !== undefined) {
    pojo["reasoning"] = msg.reasoning;
  }
  if (msg.citations !== undefined && msg.citations.length > 0) {
    pojo["citations"] = msg.citations.map((c) => ({
      url: c.url,
      title: c.title,
      start_index: c.startIndex,
      end_index: c.endIndex,
    }));
  }
  if (msg.contentParts !== undefined && msg.contentParts.length > 0) {
    pojo["content_parts"] = msg.contentParts.map(contentPartToPojo);
  }
  if (msg.toolCalls !== undefined && msg.toolCalls.length > 0) {
    pojo["tool_calls"] = msg.toolCalls.map(toolCallToPojo);
  }
  if (msg.toolCallId !== undefined) {
    pojo["tool_call_id"] = msg.toolCallId;
  }
  return pojo;
}

function contentPartToPojo(part: ContentPart): Record<string, unknown> {
  switch (part.type) {
    case "image_url": {
      return {
        type: "image_url",
        image_url: {
          url: part.imageUrl.url,
          ...(part.imageUrl.detail !== undefined && {
            detail: part.imageUrl.detail,
          }),
        },
      };
    }
    case "text": {
      return { type: "text", text: part.text };
    }
    case "video_url": {
      return {
        type: "video_url",
        video_url: definedValues({
          url: part.videoUrl.url,
          processing: part.videoUrl.processing,
        }),
      };
    }
    default: {
      part satisfies never;
      throw new Error(`Unhandled content part type: ${part}`);
    }
  }
}

function toolCallToPojo(tc: ToolCall): Record<string, unknown> {
  return {
    id: tc.id,
    type: tc.type,
    function: { name: tc.function.name, arguments: tc.function.arguments },
  };
}

export function rowScoreToNumber(scoreValue: string): number {
  return scoreValue === ScoreValue.Correct ? 1 : 0;
}

export function readResultRows(
  file: AsyncBuffer
): Promise<readonly BenchmarkResultRow[]> {
  return parquetReadObjects({ file }).then((decoded) => {
    const result = parseSchema(z.array(BenchmarkResultRowSchema), decoded);
    if (Either.isLeft(result)) {
      throw new Error(
        `Invalid benchmark result row: ${firstZodIssueMessage(result.left)}`
      );
    }
    return result.right;
  });
}

export function asyncBufferFromBytes(bytes: Uint8Array): AsyncBuffer {
  const arrayBuffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(arrayBuffer).set(bytes);
  return {
    byteLength: arrayBuffer.byteLength,
    slice: (start, end) => arrayBuffer.slice(start, end),
  };
}

export interface ChunkEpochSummary extends AggregateMetrics {
  readonly epoch: number;
}

export interface ChunkResultSummary extends UsageTotals, AggregateMetrics {
  readonly temperature: number | null;
  readonly primaryScore?: BenchmarkPrimaryScore;
  readonly epochResults: readonly ChunkEpochSummary[];
}

function parsePrimaryScore(
  raw: string | null | undefined
): BenchmarkPrimaryScore | undefined {
  if (raw === null || raw === undefined) {
    return undefined;
  }
  const parsed = parseSchema(
    z.object({ value: z.number(), weight: z.number() }),
    wrapJsonParse(raw)
  );
  return Either.isLeft(parsed) ? undefined : parsed.right;
}

function wrapJsonParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

function rowScoreValue(scoreValue: string): ScoreValue {
  switch (scoreValue) {
    case ScoreValue.Correct:
    case ScoreValue.Incorrect:
    case ScoreValue.Skipped: {
      return scoreValue;
    }
    default: {
      return ScoreValue.Skipped;
    }
  }
}

function rowsToSampleScores(
  rows: readonly BenchmarkResultRow[]
): SampleScore[] {
  return rows.map((row) => ({
    sampleId: row.sample_id,
    epoch: row.epoch,
    score: {
      value: rowScoreValue(row.score_value),
      answer: row.answer,
      explanation: "",
    },
  }));
}

export function summarizeChunkRows(
  rows: readonly BenchmarkResultRow[]
): ChunkResultSummary | null {
  const [first] = rows;
  if (first === undefined) {
    return null;
  }
  const sampleScores = rowsToSampleScores(rows);
  const metrics = aggregateScores(sampleScores);
  const byEpoch = new Map<number, SampleScore[]>();
  for (const sampleScore of sampleScores) {
    const existing = byEpoch.get(sampleScore.epoch) ?? [];
    existing.push(sampleScore);
    byEpoch.set(sampleScore.epoch, existing);
  }
  const epochResults = [...byEpoch.entries()]
    .toSorted(([a], [b]) => a - b)
    .map(([epoch, scores]) => ({ epoch, ...aggregateScores(scores) }));
  const primaryScore = parsePrimaryScore(first.primary_score);
  return {
    ...metrics,
    inputTokens: first.input_tokens,
    outputTokens: first.output_tokens,
    totalTokens: first.total_tokens,
    reasoningTokens: first.reasoning_tokens,
    totalCost: first.total_cost,
    generationTimeMs: first.generation_time_ms,
    temperature: first.temperature,
    ...(primaryScore !== undefined && { primaryScore }),
    epochResults,
  };
}
