import { unsafeNow, formatIso } from "effect/DateTime";
import type { AsyncBuffer } from "hyparquet";
import { parquetReadObjects } from "hyparquet";
import type { KeyValue, Writer } from "hyparquet-writer";
import {
  ParquetWriter,
  parquetWriteBuffer,
  schemaFromColumnData,
} from "hyparquet-writer";

import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import type { BenchmarkPrimaryScore } from "../benchmarks/types";
import type { ModelMessage, ToolCall, UsageTotals } from "../harness/core";
import { ScoreValue } from "../harness/core";
import type { AggregateMetrics, SampleScore } from "../harness/metric";
import { aggregateScores } from "../harness/metric";
import type { RunResult } from "../harness/run";
import { Either } from "../internal/either";
import { definedValues } from "../internal/guards";
import { firstZodIssueMessage, parseSchema, z } from "../internal/zod";
import {
  citationToPojo,
  contentPartToPojo,
  messagesToAtif,
} from "./messages-to-atif";
import type { BenchmarkResultRow } from "./parquet-schema";
import {
  RESULT_FORMAT_VERSION,
  RESULT_WRITER,
  BenchmarkResultRowSchema,
  ScorerTrajectorySchema,
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

export interface ResultRowsParquetMeta {
  readonly task: string;
  readonly model: string;
  readonly createdAt?: string;
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
  { name: "trajectory", type: "JSON", nullable: true },
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
  const extraScoresJson = extraScoresToJson(extraScores);
  const primaryScoreJson =
    primaryScore !== undefined ? JSON.stringify(primaryScore) : null;
  const benchmarkConfigJson =
    meta.benchmarkConfig !== undefined
      ? JSON.stringify(meta.benchmarkConfig)
      : null;
  const counts = epochCounts(sampleScores);
  const rowCtx: RowContext = {
    metrics,
    usage,
    ...counts,
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
  return writeParquet(columnData, {
    task: meta.task,
    model: meta.model,
    createdAt,
  });
}

export type ResultFileReader = () => Promise<readonly BenchmarkResultRow[]>;

export interface MergeResultFilesInput {
  readonly files: readonly ResultFileReader[];
  readonly meta: ResultRowsParquetMeta;
  readonly writer: Writer;
  readonly runLevelScores?: (result: RunResult) => readonly ExtraScore[];
}

export interface MergedResultFiles {
  readonly summary: ChunkResultSummary | null;
  readonly benchmarkConfig: string | null;
}

type MergedRunColumns = Pick<
  BenchmarkResultRow,
  | "format_version"
  | "task"
  | "model"
  | "epochs"
  | "temperature"
  | "benchmark_config"
  | "created_at"
  | "accuracy"
  | "total_questions"
  | "correct_answers"
  | "input_tokens"
  | "output_tokens"
  | "total_tokens"
  | "reasoning_tokens"
  | "total_cost"
  | "generation_time_ms"
  | "epoch_total_questions"
  | "epoch_correct_answers"
  | "extra_scores"
  | "primary_score"
>;

interface ResultFilesScan {
  readonly rowCounts: readonly number[];
  readonly runFingerprints: readonly (string | undefined)[];
  readonly sampleScores: readonly SampleScore[];
  readonly usage: UsageTotals;
  readonly primaryScore: BenchmarkPrimaryScore | undefined;
  readonly first:
    | Pick<BenchmarkResultRow, "epochs" | "temperature" | "benchmark_config">
    | undefined;
}

const MERGE_ROW_GROUP_ROWS = 100;

const RESULT_SCHEMA = schemaFromColumnData({
  columnData: COLUMN_SPECS.map((spec) => ({
    name: spec.name,
    type: spec.type,
    nullable: spec.nullable,
    data: [],
  })),
});

export async function mergeResultFilesToParquet(
  input: MergeResultFilesInput
): Promise<MergedResultFiles> {
  const { files, meta, writer } = input;
  const createdAt = meta.createdAt ?? formatIso(unsafeNow());
  const scan = await scanResultFiles(
    files,
    input.runLevelScores === undefined
      ? rowsToSampleScores
      : rowsToScoredSamples
  );
  const extraScores = input.runLevelScores?.({
    metrics: aggregateScores(scan.sampleScores),
    usage: scan.usage,
    sampleScores: scan.sampleScores,
  });
  const runColumns = mergedRunColumns(scan, {
    ...meta,
    createdAt,
    extraScoresJson: extraScoresToJson(extraScores),
  });
  const parquetWriter = new ParquetWriter({
    writer,
    schema: RESULT_SCHEMA,
    codec: "SNAPPY",
    kvMetadata: resultKvMetadata({ ...meta, createdAt }),
  });
  let sampleOffset = 0;
  for (const [index, readFile] of files.entries()) {
    const expectedRows = scan.rowCounts[index] ?? 0;
    if (expectedRows === 0) {
      continue;
    }
    const rows = await readFile();
    const unchanged =
      rows.length === expectedRows &&
      runFingerprint(rows[0]) === scan.runFingerprints[index] &&
      rows.every((row, rowIndex) =>
        sameSampleScore(row, scan.sampleScores[sampleOffset + rowIndex])
      );
    if (!unchanged) {
      throw new Error(`Result file ${index} changed between merge passes`);
    }
    sampleOffset += expectedRows;
    const mergedRows = rows.map((row) => ({ ...row, ...runColumns }));
    await parquetWriter.write({
      columnData: COLUMN_SPECS.map((spec) => ({
        name: spec.name,
        data: mergedRows.map((row) => row[spec.name] ?? null),
      })),
      rowGroupSize: MERGE_ROW_GROUP_ROWS,
    });
  }
  await parquetWriter.finish();
  return {
    summary:
      scan.first === undefined
        ? null
        : summarizeSampleScores(scan.sampleScores, {
            ...scan.usage,
            temperature: scan.first.temperature,
            ...definedValues({ primaryScore: scan.primaryScore }),
          }),
    benchmarkConfig: runColumns.benchmark_config ?? null,
  };
}

async function scanResultFiles(
  files: readonly ResultFileReader[],
  toSampleScores: (rows: readonly BenchmarkResultRow[]) => SampleScore[]
): Promise<ResultFilesScan> {
  const rowCounts: number[] = [];
  const runFingerprints: (string | undefined)[] = [];
  const sampleScores: SampleScore[] = [];
  const usage = {
    inputTokens: 0,
    outputTokens: 0,
    totalTokens: 0,
    reasoningTokens: 0,
    totalCost: 0,
    generationTimeMs: 0,
  };
  let first: ResultFilesScan["first"];
  let primaryScoreValue = 0;
  let primaryScoreWeight = 0;
  let hasPrimaryScore = false;
  for (const readFile of files) {
    const rows = await readFile();
    rowCounts.push(rows.length);
    runFingerprints.push(runFingerprint(rows[0]));
    const [row] = rows;
    if (row === undefined) {
      continue;
    }
    first ??= {
      epochs: row.epochs,
      temperature: row.temperature,
      benchmark_config: row.benchmark_config,
    };
    for (const sampleScore of toSampleScores(rows)) {
      sampleScores.push(sampleScore);
    }
    const parsedPrimaryScore = parsePrimaryScore(row.primary_score);
    hasPrimaryScore ||= parsedPrimaryScore !== undefined;
    const primaryScore = parsedPrimaryScore ?? {
      value: row.accuracy,
      weight: row.total_questions,
    };
    primaryScoreValue += primaryScore.value * primaryScore.weight;
    primaryScoreWeight += primaryScore.weight;
    usage.inputTokens += row.input_tokens;
    usage.outputTokens += row.output_tokens;
    usage.totalTokens += row.total_tokens;
    usage.reasoningTokens += row.reasoning_tokens;
    usage.totalCost += row.total_cost;
    usage.generationTimeMs += row.generation_time_ms;
  }
  return {
    rowCounts,
    runFingerprints,
    sampleScores,
    usage,
    first,
    primaryScore:
      hasPrimaryScore && primaryScoreWeight > 0
        ? {
            value: primaryScoreValue / primaryScoreWeight,
            weight: primaryScoreWeight,
          }
        : undefined,
  };
}

function runFingerprint(
  row: BenchmarkResultRow | undefined
): string | undefined {
  if (row === undefined) {
    return undefined;
  }
  return JSON.stringify([
    row.epochs,
    row.temperature,
    row.benchmark_config ?? null,
    row.accuracy,
    row.total_questions,
    row.primary_score,
    row.input_tokens,
    row.output_tokens,
    row.total_tokens,
    row.reasoning_tokens,
    row.total_cost,
    row.generation_time_ms,
  ]);
}

function sameSampleScore(
  row: BenchmarkResultRow,
  sampleScore: SampleScore | undefined
): boolean {
  return (
    sampleScore !== undefined &&
    row.sample_id === sampleScore.sampleId &&
    row.epoch === sampleScore.epoch &&
    rowScoreValue(row.score_value) === sampleScore.score.value
  );
}

function mergedRunColumns(
  scan: ResultFilesScan,
  meta: ResultRowsParquetMeta & {
    readonly createdAt: string;
    readonly extraScoresJson: string | null;
  }
): MergedRunColumns {
  const metrics = aggregateScores(scan.sampleScores);
  const counts = epochCounts(scan.sampleScores);
  return {
    format_version: RESULT_FORMAT_VERSION,
    task: meta.task,
    model: meta.model,
    epochs: scan.first?.epochs ?? 0,
    temperature: scan.first?.temperature ?? null,
    benchmark_config: scan.first?.benchmark_config ?? null,
    created_at: meta.createdAt,
    accuracy: scan.primaryScore?.value ?? metrics.accuracy,
    total_questions: metrics.totalQuestions,
    correct_answers: metrics.correctAnswers,
    input_tokens: scan.usage.inputTokens,
    output_tokens: scan.usage.outputTokens,
    total_tokens: scan.usage.totalTokens,
    reasoning_tokens: scan.usage.reasoningTokens,
    total_cost: scan.usage.totalCost,
    generation_time_ms: scan.usage.generationTimeMs,
    epoch_total_questions: counts.epochTotalQuestions,
    epoch_correct_answers: counts.epochCorrectAnswers,
    extra_scores: meta.extraScoresJson,
    primary_score: scan.primaryScore ? JSON.stringify(scan.primaryScore) : null,
  };
}

function epochCounts(sampleScores: readonly SampleScore[]): {
  readonly epochTotalQuestions: number;
  readonly epochCorrectAnswers: number;
} {
  return {
    epochTotalQuestions: sampleScores.filter(
      (sampleScore) => sampleScore.score.value !== ScoreValue.Skipped
    ).length,
    epochCorrectAnswers: sampleScores.filter(
      (sampleScore) => sampleScore.score.value === ScoreValue.Correct
    ).length,
  };
}

function writeParquet(
  columnData: {
    readonly name: string;
    readonly type: ColumnSpec["type"];
    readonly nullable: boolean;
    readonly data: unknown[];
  }[],
  meta: ResultRowsParquetMeta
): Buffer {
  const createdAt = meta.createdAt ?? formatIso(unsafeNow());
  const arrayBuffer = parquetWriteBuffer({
    columnData,
    codec: "SNAPPY",
    kvMetadata: resultKvMetadata({ ...meta, createdAt }),
  });
  return Buffer.from(arrayBuffer);
}

function resultKvMetadata(
  meta: ResultRowsParquetMeta & { readonly createdAt: string }
): KeyValue[] {
  return [
    { key: "writer", value: RESULT_WRITER },
    { key: "schema_version", value: String(RESULT_FORMAT_VERSION) },
    { key: "task", value: meta.task },
    { key: "model", value: meta.model },
    { key: "created_at", value: meta.createdAt },
  ];
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
    case "trajectory": {
      const trajectory = messagesToAtif({
        messages: s.messages ?? [],
        model: ctx.meta.model,
        scorerTrajectory: s.score.trajectory,
      });
      return trajectory === null ? null : JSON.stringify(trajectory);
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
    pojo["citations"] = msg.citations.map(citationToPojo);
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
      answer: null,
      explanation: "",
    },
  }));
}

function rowsToScoredSamples(
  rows: readonly BenchmarkResultRow[]
): SampleScore[] {
  return rows.map((row) =>
    definedValues({
      sampleId: row.sample_id,
      epoch: row.epoch,
      score: definedValues({
        value: rowScoreValue(row.score_value),
        answer: row.answer,
        explanation: row.explanation ?? "",
        trajectory: parseJsonColumn(
          ScorerTrajectorySchema,
          row.scorer_trajectory
        ),
      }),
      metadata: parseJsonColumn(
        z.record(z.string(), z.unknown()),
        row.metadata
      ),
    })
  );
}

function parseJsonColumn<T>(
  schema: z.ZodType<T>,
  raw: string | null | undefined
): T | undefined {
  if (raw === null || raw === undefined) {
    return undefined;
  }
  const parsed = parseSchema(schema, JSON.parse(raw));
  if (Either.isLeft(parsed)) {
    throw new Error(
      `Invalid result column: ${firstZodIssueMessage(parsed.left)}`
    );
  }
  return parsed.right;
}

function extraScoresToJson(
  extraScores: readonly ExtraScore[] | undefined
): string | null {
  return extraScores !== undefined && extraScores.length > 0
    ? JSON.stringify(extraScores)
    : null;
}

export function summarizeChunkRows(
  rows: readonly BenchmarkResultRow[]
): ChunkResultSummary | null {
  const [first] = rows;
  if (first === undefined) {
    return null;
  }
  return summarizeSampleScores(rowsToSampleScores(rows), {
    inputTokens: first.input_tokens,
    outputTokens: first.output_tokens,
    totalTokens: first.total_tokens,
    reasoningTokens: first.reasoning_tokens,
    totalCost: first.total_cost,
    generationTimeMs: first.generation_time_ms,
    temperature: first.temperature,
    ...definedValues({
      primaryScore: parsePrimaryScore(first.primary_score),
    }),
  });
}

function summarizeSampleScores(
  sampleScores: readonly SampleScore[],
  run: Omit<ChunkResultSummary, keyof AggregateMetrics | "epochResults">
): ChunkResultSummary {
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
  return { ...metrics, ...run, epochResults };
}
