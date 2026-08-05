import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { MessageRole, ScoreValue } from "./core";
import type {
  CompletedSampleEntry,
  SampleResultEnvelope,
} from "./sample-result-store";
import {
  decodeSampleResultEntry,
  encodeSampleResultRecord,
  NOOP_SAMPLE_RESULT_STORE,
  SAMPLE_RESULT_FORMAT_VERSION,
  SampleResultRecordSchema,
} from "./sample-result-store";

const ENVELOPE: SampleResultEnvelope = {
  parentWorkflowId: "parent-1",
  childWorkflowId: "child-1",
  chunkIndex: 3,
  benchmarkId: "gpqa_diamond",
  model: "openai/gpt-5",
};

const ENTRY: CompletedSampleEntry = {
  sampleIndex: 42,
  epoch: 1,
  sampleScore: {
    sampleId: "s-42",
    epoch: 1,
    score: {
      value: ScoreValue.Correct,
      answer: "B",
      explanation: "matched target",
    },
    messages: [{ role: MessageRole.Assistant, content: "Answer: B" }],
    responseItems: [{ type: "message", id: "item-1" }],
    generationIds: ["gen-1"],
    metadata: { difficulty: "hard" },
    input: "Q42",
    target: "B",
  },
  usage: {
    inputTokens: 10,
    outputTokens: 5,
    totalTokens: 15,
    reasoningTokens: 2,
    totalCost: 0.001,
  },
  generationTimeMs: 1234,
};

describe("encodeSampleResultRecord", () => {
  it("stamps the pinned envelope fields and absolute sample index", () => {
    const record = encodeSampleResultRecord(ENVELOPE, ENTRY);

    expect(record.format_version).toBe(SAMPLE_RESULT_FORMAT_VERSION);
    expect(record.parent_workflow_id).toBe("parent-1");
    expect(record.child_workflow_id).toBe("child-1");
    expect(record.chunk_index).toBe(3);
    expect(record.benchmark_id).toBe("gpqa_diamond");
    expect(record.model).toBe("openai/gpt-5");
    expect(record.sample_id).toBe("s-42");
    expect(record.sample_index).toBe(42);
    expect(record.epoch).toBe(1);
    expect(record.usage).toEqual({
      input_tokens: 10,
      output_tokens: 5,
      total_tokens: 15,
      reasoning_tokens: 2,
      total_cost: 0.001,
    });
    expect(record.generation_time_ms).toBe(1234);
    expect(Date.parse(record.created_at)).not.toBeNaN();
  });

  it("nulls usage and generation time when the outcome carried neither", () => {
    const record = encodeSampleResultRecord(ENVELOPE, {
      sampleIndex: 0,
      epoch: 0,
      sampleScore: {
        sampleId: "s-0",
        epoch: 0,
        score: {
          value: ScoreValue.Skipped,
          answer: null,
          explanation: "rate limited",
        },
      },
    });

    expect(record.usage).toBeNull();
    expect(record.generation_time_ms).toBeNull();
  });

  it("defaults missing per-call usage fields to zero", () => {
    const record = encodeSampleResultRecord(ENVELOPE, {
      sampleIndex: 1,
      epoch: 0,
      sampleScore: {
        sampleId: "s-1",
        epoch: 0,
        score: {
          value: ScoreValue.Incorrect,
          answer: "A",
          explanation: "wrong",
        },
      },
      usage: { totalCost: 0.5 },
    });

    expect(record.usage).toEqual({
      input_tokens: 0,
      output_tokens: 0,
      total_tokens: 0,
      reasoning_tokens: 0,
      total_cost: 0.5,
    });
  });
});

describe("sample result record round-trip", () => {
  it("restores the entry through JSON and Zod validation", () => {
    const record = encodeSampleResultRecord(ENVELOPE, ENTRY);
    const parsed = parseSchema(
      SampleResultRecordSchema,
      JSON.parse(JSON.stringify(record))
    );
    assertRight(parsed);

    expect(decodeSampleResultEntry(parsed.right)).toEqual(ENTRY);
  });

  it("round-trips the degraded marker and omits it on genuine evaluations", () => {
    const degraded = encodeSampleResultRecord(ENVELOPE, {
      ...ENTRY,
      degraded: true,
    });
    expect(degraded.degraded).toBe(true);
    const parsed = parseSchema(
      SampleResultRecordSchema,
      JSON.parse(JSON.stringify(degraded))
    );
    assertRight(parsed);
    expect(decodeSampleResultEntry(parsed.right)).toEqual({
      ...ENTRY,
      degraded: true,
    });

    expect("degraded" in encodeSampleResultRecord(ENVELOPE, ENTRY)).toBe(false);
  });

  it("rejects a record whose format version is not the pinned one", () => {
    const record = encodeSampleResultRecord(ENVELOPE, ENTRY);
    const parsed = parseSchema(SampleResultRecordSchema, {
      ...record,
      format_version: 2,
    });

    assertLeft(parsed);
  });

  it("rejects a record missing the sample index", () => {
    const { sample_index: _sampleIndex, ...withoutIndex } =
      encodeSampleResultRecord(ENVELOPE, ENTRY);
    const parsed = parseSchema(SampleResultRecordSchema, withoutIndex);

    assertLeft(parsed);
  });
});

describe("NOOP_SAMPLE_RESULT_STORE", () => {
  it("accepts writes and reports nothing completed", async () => {
    await NOOP_SAMPLE_RESULT_STORE.write(ENTRY);

    expect(await NOOP_SAMPLE_RESULT_STORE.list({})).toEqual([]);
  });
});
