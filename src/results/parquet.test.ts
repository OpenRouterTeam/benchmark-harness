import { beforeAll, describe, expect, it } from "bun:test";
import assert from "node:assert";

import type { AsyncBuffer } from "hyparquet";
import { parquetMetadata } from "hyparquet";

import type { ModelMessage, ResponseItem } from "../harness/core";
import { MessageRole, ScoreValue } from "../harness/core";
import type { SampleScore } from "../harness/metric";
import { assertRight, assertLeft } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import {
  readResultRows,
  runResultToParquet,
  rowScoreToNumber,
  summarizeChunkRows,
} from "./parquet";
import type { BenchmarkResultRow } from "./parquet-schema";
import {
  RESULT_FORMAT_VERSION,
  RESULT_WRITER,
  BenchmarkResultRowSchema,
  ScorerTrajectorySchema,
} from "./parquet-schema";

const USAGE = {
  inputTokens: 100,
  outputTokens: 50,
  totalTokens: 150,
  reasoningTokens: 0,
  totalCost: 0.01,
  generationTimeMs: 1000,
} as const;

const SAMPLE_SCORES: readonly SampleScore[] = [
  {
    sampleId: "s0",
    epoch: 0,
    score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
    input: "What is 2+2?",
    target: "B",
  },
  {
    sampleId: "s0",
    epoch: 1,
    score: { value: ScoreValue.Incorrect, answer: "A", explanation: "guessed" },
    input: "What is 2+2?",
    target: "B",
  },
  {
    sampleId: "s1",
    epoch: 0,
    score: { value: ScoreValue.Correct, answer: "C", explanation: "" },
    input: "Capital of France?",
    target: "C",
  },
];

const METRICS = {
  accuracy: 0.75,
  totalQuestions: 2,
  correctAnswers: 2,
  skippedQuestions: 0,
} as const;

const RESULT = {
  metrics: METRICS,
  usage: USAGE,
  sampleScores: SAMPLE_SCORES,
} as const;

const META = {
  task: "gpqa_diamond",
  model: "openai/gpt-4o-mini",
  epochs: 2,
  temperature: 0.5,
  createdAt: "2026-06-27T00:00:00.000Z",
  benchmarkConfig: {
    benchmarkId: "gpqa_diamond",
    model: "openai/gpt-4o-mini",
    temperature: 0.5,
    maxTokens: 1024,
    reasoningEffort: "high",
    endpointId: "11111111-2222-3333-4444-555555555555",
  },
} as const;

function asyncBufferFromArrayBuffer(buf: ArrayBuffer): AsyncBuffer {
  return {
    byteLength: buf.byteLength,
    slice: (start, end) => buf.slice(start, end),
  };
}

function bufferToArrayBuffer(buf: Buffer): ArrayBuffer {
  const ab = new ArrayBuffer(buf.byteLength);
  new Uint8Array(ab).set(buf);
  return ab;
}

function readRows(buf: Buffer): Promise<readonly BenchmarkResultRow[]> {
  const file = asyncBufferFromArrayBuffer(bufferToArrayBuffer(buf));
  return readResultRows(file);
}
describe("runResultToParquet", () => {
  const buffer = runResultToParquet({ result: RESULT, meta: META });
  let rows: readonly BenchmarkResultRow[];
  beforeAll(async () => {
    rows = await readRows(buffer);
  });
  it("produces valid parquet (PAR1 magic bytes)", () => {
    expect(buffer.subarray(0, 4)).toEqual(Buffer.from("PAR1"));
  });
  it("emits one row per (sample, epoch)", () => {
    expect(rows).toHaveLength(3);
    expect(
      rows.map((r) => `${r.sample_id}_epoch_${r.epoch}`).toSorted()
    ).toEqual(["s0_epoch_0", "s0_epoch_1", "s1_epoch_0"]);
  });
  it("denormalizes run-level scalars onto every row", () => {
    for (const row of rows) {
      expect(row.format_version).toBe(RESULT_FORMAT_VERSION);
      expect(row.task).toBe("gpqa_diamond");
      expect(row.model).toBe("openai/gpt-4o-mini");
      expect(row.epochs).toBe(2);
      expect(row.temperature).toBe(0.5);
      expect(row.created_at).toBe("2026-06-27T00:00:00.000Z");
      const parsedConfig: Record<string, unknown> = JSON.parse(
        row.benchmark_config!
      );
      expect(parsedConfig).toEqual(META.benchmarkConfig);
      expect(row.accuracy).toBe(0.75);
      expect(row.total_questions).toBe(2);
      expect(row.correct_answers).toBe(2);
      expect(row.input_tokens).toBe(100);
      expect(row.output_tokens).toBe(50);
      expect(row.total_tokens).toBe(150);
      expect(row.reasoning_tokens).toBe(0);
      expect(row.total_cost).toBe(0.01);
      expect(row.generation_time_ms).toBe(1000);
      expect(row.epoch_total_questions).toBe(3);
      expect(row.epoch_correct_answers).toBe(2);
    }
  });
  it("serializes per-sample score, answer, explanation, input, target", () => {
    const correct = rows.find((r) => r.sample_id === "s0" && r.epoch === 0)!;
    expect(correct.score_value).toBe("C");
    expect(correct.answer).toBe("B");
    expect(correct.explanation).toBeNull();
    expect(correct.input).toBe("What is 2+2?");
    expect(correct.target).toBe("B");
    const incorrect = rows.find((r) => r.sample_id === "s0" && r.epoch === 1)!;
    expect(incorrect.score_value).toBe("I");
    expect(incorrect.explanation).toBe("guessed");
  });
  it("serializes the scorer trajectory as a nullable JSON column", async () => {
    const trajectory = {
      kind: "verifier_log",
      log: "P2P 3/3 pass 0 fail\nF2P 20/20 pass 0 fail",
    } as const;
    const bufferWithTrajectory = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: {
              value: ScoreValue.Correct,
              answer: "B",
              explanation: "reward=1",
              trajectory,
            },
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const trajectoryRows = await readRows(bufferWithTrajectory);
    const parsed = parseSchema(
      ScorerTrajectorySchema,
      JSON.parse(trajectoryRows[0]!.scorer_trajectory!)
    );
    assertRight(parsed);
    expect(parsed.right).toEqual(trajectory);
  });
  it("round-trips a judge_runs scorer trajectory through ScorerTrajectorySchema", async () => {
    const trajectory = {
      kind: "judge_runs",
      runs: [
        {
          runNum: 1,
          status: "ok",
          verdicts: [{ id: "c1", verdict: "MET", weight: 2 }],
        },
      ],
    } as const;
    const bufferWithJudgeRuns = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: {
              value: ScoreValue.Correct,
              answer: "B",
              explanation: "",
              trajectory,
            },
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const judgeRows = await readRows(bufferWithJudgeRuns);
    const parsed = parseSchema(
      ScorerTrajectorySchema,
      JSON.parse(judgeRows[0]!.scorer_trajectory!)
    );
    assertRight(parsed);
    expect(parsed.right).toEqual(trajectory);
  });
  it("writes null scorer_trajectory when the scorer recorded none", () => {
    for (const row of rows) {
      expect(row.scorer_trajectory).toBeNull();
    }
  });
  it("writes null response_items and generation_ids when the solver recorded none", () => {
    for (const row of rows) {
      expect(row.response_items).toBeNull();
      expect(row.request_body).toBeNull();
      expect(row.generation_ids).toBeNull();
    }
  });
  it("round-trips the effective request body as JSON", async () => {
    const requestBody = {
      model: "openai/gpt-5.4-nano",
      maxOutputTokens: 128000,
      provider: {
        order: ["openai", "azure"],
        only: ["openai", "azure"],
        allowFallbacks: false,
      },
      tools: [
        {
          type: "openrouter:web_search",
          parameters: { excludedDomains: ["example.com"] },
        },
      ],
    };
    const bufferWithRequest = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            requestBody,
          },
        ],
      },
      meta: META,
    });
    const requestRows = await readRows(bufferWithRequest);
    expect(JSON.parse(requestRows[0]!.request_body!)).toEqual(requestBody);
  });
  it("serializes generation ids as a JSON column", async () => {
    const bufferWithIds = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            generationIds: ["gen-1", "gen-2"],
          },
        ],
      },
      meta: META,
    });
    const idRows = await readRows(bufferWithIds);
    expect(JSON.parse(idRows[0]!.generation_ids!)).toEqual(["gen-1", "gen-2"]);
  });
  it("serializes response_items as a JSON column preserving raw advisor items", async () => {
    const advisorItem = {
      type: "openrouter:advisor",
      id: "st_tmp_abc123",
      status: "completed",
      model: "openai/gpt-5.6-sol",
      prompt: "Sanity check: what is 2+2?",
      advice: "Confirmed: 2 + 2 = 4.",
      instance_name: "openrouter_advisor__1",
    } as const;
    const functionCall = {
      type: "function_call",
      call_id: "call-1",
      name: "bash",
      arguments: '{"command":"echo hi"}',
    } as const;
    const responseItems: readonly ResponseItem[] = [
      { role: "user", content: "What is 2+2?" },
      advisorItem,
      functionCall,
      { type: "function_call_output", call_id: "call-1", output: "hi\n" },
    ];
    const bufferWithItems = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            responseItems,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const itemRows = await readRows(bufferWithItems);
    const parsed: Record<string, unknown>[] = JSON.parse(
      itemRows[0]!.response_items!
    );
    expect(parsed).toHaveLength(4);
    expect(parsed[1]).toEqual(advisorItem);
    expect(parsed[2]).toEqual(functionCall);
  });
  it("rowScoreToNumber maps the enum to 1/0", () => {
    expect(rowScoreToNumber("C")).toBe(1);
    expect(rowScoreToNumber("I")).toBe(0);
  });
  it("serializes message trajectories as a JSON column", async () => {
    const messages: readonly ModelMessage[] = [
      { role: MessageRole.System, content: "You are a helpful assistant." },
      { role: MessageRole.User, content: "What is 2+2?" },
      { role: MessageRole.Assistant, content: "Answer: B" },
    ];
    const bufferWithTrajectory = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            messages,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const trajectoryRows = await readRows(bufferWithTrajectory);
    const parsed: {
      role: string;
      content: string;
    }[] = JSON.parse(trajectoryRows[0]!.messages!);
    expect(parsed).toEqual([
      { role: "system", content: "You are a helpful assistant." },
      { role: "user", content: "What is 2+2?" },
      { role: "assistant", content: "Answer: B" },
    ]);
  });
  it("serializes tool calls and tool_call_id in the messages JSON", async () => {
    const messages: readonly ModelMessage[] = [
      {
        role: MessageRole.Assistant,
        content: "",
        toolCalls: [
          {
            id: "call_1",
            type: "function",
            function: { name: "get_weather", arguments: '{"city":"SF"}' },
          },
        ],
      },
      { role: MessageRole.Tool, content: '{"temp":72}', toolCallId: "call_1" },
    ];
    const bufferWithTools = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            messages,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const toolRows = await readRows(bufferWithTools);
    const parsed: Record<string, unknown>[] = JSON.parse(
      toolRows[0]!.messages!
    );
    expect(parsed[0]?.["tool_calls"]).toEqual([
      {
        id: "call_1",
        type: "function",
        function: { name: "get_weather", arguments: '{"city":"SF"}' },
      },
    ]);
    expect(parsed[1]?.["tool_call_id"]).toBe("call_1");
    expect(parsed[1]).not.toHaveProperty("tool_calls");
  });
  it("serializes assistant reasoning traces in the messages JSON", async () => {
    const messages: readonly ModelMessage[] = [
      {
        role: MessageRole.Assistant,
        content: "Answer: B",
        reasoning: "Step 1: ...",
      },
    ];
    const bufferWithReasoning = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            messages,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const reasoningRows = await readRows(bufferWithReasoning);
    const parsed: Record<string, unknown>[] = JSON.parse(
      reasoningRows[0]!.messages!
    );
    expect(parsed[0]?.["reasoning"]).toBe("Step 1: ...");
  });
  it("serializes multimodal content parts (image_url) in the messages JSON", async () => {
    const messages: readonly ModelMessage[] = [
      {
        role: MessageRole.User,
        content: "What is this image?",
        contentParts: [
          { type: "text", text: "What is this image?" },
          {
            type: "image_url",
            imageUrl: { url: "https://example.com/img.png", detail: "high" },
          },
        ],
      },
    ];
    const bufferWithParts = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            messages,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const partsRows = await readRows(bufferWithParts);
    const parsed: Record<string, unknown>[] = JSON.parse(
      partsRows[0]!.messages!
    );
    expect(parsed[0]?.["content_parts"]).toEqual([
      { type: "text", text: "What is this image?" },
      {
        type: "image_url",
        image_url: { url: "https://example.com/img.png", detail: "high" },
      },
    ]);
  });
  it("serializes assistant citations (camelCase→snake_case) in the messages JSON", async () => {
    const messages: readonly ModelMessage[] = [
      {
        role: MessageRole.Assistant,
        content: "Answer based on sources.",
        citations: [
          {
            url: "https://example.com/a",
            title: "Source A",
            startIndex: 0,
            endIndex: 10,
          },
          {
            url: "https://example.com/b",
            title: "Source B",
            startIndex: 11,
            endIndex: 20,
          },
        ],
      },
    ];
    const bufferWithCitations = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            messages,
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const citationRows = await readRows(bufferWithCitations);
    const parsed: Record<string, unknown>[] = JSON.parse(
      citationRows[0]!.messages!
    );
    expect(parsed[0]?.["citations"]).toEqual([
      {
        url: "https://example.com/a",
        title: "Source A",
        start_index: 0,
        end_index: 10,
      },
      {
        url: "https://example.com/b",
        title: "Source B",
        start_index: 11,
        end_index: 20,
      },
    ]);
  });
  it("serializes per-sample metadata as a JSON column", async () => {
    const bufferWithMeta = runResultToParquet({
      result: {
        metrics: METRICS,
        usage: USAGE,
        sampleScores: [
          {
            sampleId: "s0",
            epoch: 0,
            score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
            metadata: { fusionAnalysis: { normalized: 72.5 }, passRate: 80 },
            input: "q",
            target: "B",
          },
        ],
      },
      meta: META,
    });
    const metaRows = await readRows(bufferWithMeta);
    expect(JSON.parse(metaRows[0]!.metadata!)).toEqual({
      fusionAnalysis: { normalized: 72.5 },
      passRate: 80,
    });
  });
  it("omits extra_scores column value when no run-level extra scores", () => {
    for (const row of rows) {
      expect(row.extra_scores).toBeNull();
    }
  });
  it("serializes run-level extra_scores as a JSON column", async () => {
    const bufferWithExtra = runResultToParquet({
      result: RESULT,
      meta: META,
      extraScores: [
        {
          name: "draco",
          metrics: { normalized: { value: 72.5 }, pass_rate: { value: 80 } },
        },
      ],
    });
    const extraRows = await readRows(bufferWithExtra);
    for (const row of extraRows) {
      const parsed = JSON.parse(row.extra_scores!);
      expect(parsed).toEqual([
        {
          name: "draco",
          metrics: { normalized: { value: 72.5 }, pass_rate: { value: 80 } },
        },
      ]);
    }
  });
  it("writes writer/schema_version/task/model KV metadata", () => {
    const metadata = parquetMetadata(bufferToArrayBuffer(buffer));
    const kv = metadata.key_value_metadata ?? [];
    const byKey = Object.fromEntries(kv.map((k) => [k.key, k.value]));
    expect(byKey["writer"]).toBe(RESULT_WRITER);
    expect(byKey["schema_version"]).toBe("1");
    expect(byKey["task"]).toBe("gpqa_diamond");
    expect(byKey["model"]).toBe("openai/gpt-4o-mini");
  });
});
describe("BenchmarkResultRowSchema", () => {
  it("parses a valid row object", () => {
    const parsed = parseSchema(BenchmarkResultRowSchema, {
      format_version: 1,
      task: "t",
      model: "m",
      epochs: 1,
      temperature: null,
      benchmark_config: null,
      created_at: "2026-06-27T00:00:00.000Z",
      accuracy: 0.5,
      total_questions: 2,
      correct_answers: 1,
      input_tokens: 1,
      output_tokens: 1,
      total_tokens: 2,
      reasoning_tokens: 0,
      total_cost: 0,
      generation_time_ms: 0,
      extra_scores: null,
      sample_id: "s0",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: null,
      messages: null,
      metadata: null,
    });
    assertRight(parsed);
  });
  it("parses a v1 row missing the benchmark_config column (back-compat)", () => {
    const parsed = parseSchema(BenchmarkResultRowSchema, {
      format_version: 1,
      task: "t",
      model: "m",
      epochs: 1,
      temperature: null,
      created_at: "2026-06-27T00:00:00.000Z",
      accuracy: 0.5,
      total_questions: 2,
      correct_answers: 1,
      input_tokens: 1,
      output_tokens: 1,
      total_tokens: 2,
      reasoning_tokens: 0,
      total_cost: 0,
      generation_time_ms: 0,
      extra_scores: null,
      sample_id: "s0",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: null,
      messages: null,
      metadata: null,
    });
    assertRight(parsed);
    expect(parsed.right.benchmark_config).toBeUndefined();
    expect(parsed.right.scorer_trajectory).toBeUndefined();
    expect(parsed.right.response_items).toBeUndefined();
    expect(parsed.right.request_body).toBeUndefined();
    expect(parsed.right.generation_ids).toBeUndefined();
  });
  it("parses a row with response_items present", () => {
    const parsed = parseSchema(BenchmarkResultRowSchema, {
      format_version: 1,
      task: "t",
      model: "m",
      epochs: 1,
      temperature: null,
      benchmark_config: null,
      created_at: "2026-06-27T00:00:00.000Z",
      accuracy: 0.5,
      total_questions: 2,
      correct_answers: 1,
      input_tokens: 1,
      output_tokens: 1,
      total_tokens: 2,
      reasoning_tokens: 0,
      total_cost: 0,
      generation_time_ms: 0,
      extra_scores: null,
      sample_id: "s0",
      epoch: 0,
      input: null,
      target: null,
      score_value: "C",
      answer: null,
      explanation: null,
      response_items: '[{"type":"openrouter:advisor","advice":"hi"}]',
      generation_ids: '["gen-1"]',
      messages: null,
      metadata: null,
    });
    assertRight(parsed);
    expect(parsed.right.response_items).toBe(
      '[{"type":"openrouter:advisor","advice":"hi"}]'
    );
    expect(parsed.right.generation_ids).toBe('["gen-1"]');
  });
  it("rejects a row missing required typed columns", () => {
    const parsed = parseSchema(BenchmarkResultRowSchema, { sample_id: "s0" });
    assertLeft(parsed);
  });
});
describe("summarizeChunkRows", () => {
  it("returns null for an empty file", () => {
    expect(summarizeChunkRows([])).toBeNull();
  });
  it("recovers run-level metrics and usage from a round-tripped chunk parquet", async () => {
    const rows = await readRows(
      runResultToParquet({ result: RESULT, meta: META })
    );
    const summary = summarizeChunkRows(rows);
    assert(summary);
    expect(summary.accuracy).toBe(METRICS.accuracy);
    expect(summary.totalQuestions).toBe(METRICS.totalQuestions);
    expect(summary.correctAnswers).toBe(METRICS.correctAnswers);
    expect(summary.skippedQuestions).toBe(0);
    expect(summary.inputTokens).toBe(USAGE.inputTokens);
    expect(summary.outputTokens).toBe(USAGE.outputTokens);
    expect(summary.totalCost).toBe(USAGE.totalCost);
    expect(summary.generationTimeMs).toBe(USAGE.generationTimeMs);
    expect(summary.temperature).toBe(META.temperature);
  });
  it("round-trips a custom primaryScore so a resumed chunk keeps its exact weight", async () => {
    const rows = await readRows(
      runResultToParquet({
        result: RESULT,
        meta: META,
        primaryScore: { value: 0.7, weight: 2 },
      })
    );
    const summary = summarizeChunkRows(rows);
    assert(summary);
    assert(summary.primaryScore);
    expect(summary.primaryScore).toEqual({ value: 0.7, weight: 2 });
  });
  it("leaves primaryScore undefined when the file has none", async () => {
    const rows = await readRows(
      runResultToParquet({ result: RESULT, meta: META })
    );
    const summary = summarizeChunkRows(rows);
    assert(summary);
    expect(summary.primaryScore).toBeUndefined();
  });
  it("groups per-epoch metrics", async () => {
    const rows = await readRows(
      runResultToParquet({ result: RESULT, meta: META })
    );
    const summary = summarizeChunkRows(rows);
    assert(summary);
    expect(summary.epochResults.map((e) => e.epoch)).toEqual([0, 1]);
    const [epoch0, epoch1] = summary.epochResults;
    assert(epoch0);
    assert(epoch1);
    expect(epoch0.totalQuestions).toBe(2);
    expect(epoch0.correctAnswers).toBe(2);
    expect(epoch1.totalQuestions).toBe(1);
    expect(epoch1.correctAnswers).toBe(0);
  });
  it("counts fully-skipped samples toward skippedQuestions, not accuracy", async () => {
    const skippedResult = {
      metrics: {
        accuracy: 0,
        totalQuestions: 0,
        correctAnswers: 0,
        skippedQuestions: 1,
      },
      usage: USAGE,
      sampleScores: [
        {
          sampleId: "s-skip",
          epoch: 0,
          score: { value: ScoreValue.Skipped, answer: null, explanation: "" },
        },
      ],
    } as const;
    const rows = await readRows(
      runResultToParquet({ result: skippedResult, meta: META })
    );
    const summary = summarizeChunkRows(rows);
    assert(summary);
    expect(summary.totalQuestions).toBe(0);
    expect(summary.skippedQuestions).toBe(1);
    expect(summary.accuracy).toBe(0);
  });
});
