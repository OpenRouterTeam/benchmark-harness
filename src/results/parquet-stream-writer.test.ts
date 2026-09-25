import { describe, expect, it } from "bun:test";

import { ByteWriter } from "hyparquet-writer";

import { ScoreValue } from "../harness/core";
import {
  asyncBufferFromBytes,
  mergeResultFilesToParquet,
  readResultRows,
  runResultToParquet,
} from "./parquet";
import { parquetStreamWriter } from "./parquet-stream-writer";

const META = {
  task: "gpqa_diamond",
  model: "openai/gpt-4o-mini",
  epochs: 1,
  createdAt: "2026-01-01T00:00:00.000Z",
} as const;

function resultRows(sampleCount: number): ReturnType<typeof readResultRows> {
  const sampleScores = Array.from({ length: sampleCount }, (_, index) => ({
    sampleId: `s${index}`,
    epoch: 0,
    score: {
      value: index % 2 === 0 ? ScoreValue.Correct : ScoreValue.Incorrect,
      answer: "A".repeat(200),
      explanation: "",
    },
  }));
  const bytes = runResultToParquet({
    result: {
      metrics: {
        accuracy: 0.5,
        totalQuestions: sampleCount,
        correctAnswers: Math.ceil(sampleCount / 2),
        skippedQuestions: 0,
      },
      usage: {
        inputTokens: 1,
        outputTokens: 1,
        totalTokens: 2,
        reasoningTokens: 0,
        totalCost: 0,
        generationTimeMs: 1,
      },
      sampleScores,
    },
    meta: META,
  });
  return readResultRows(asyncBufferFromBytes(bytes));
}

describe("parquetStreamWriter", () => {
  it("streams the same bytes an in-memory merge produces, in bounded chunks", async () => {
    const rows = await resultRows(250);
    const files = [rows, rows, rows].map((file) => () => Promise.resolve(file));
    const meta = {
      task: META.task,
      model: META.model,
      createdAt: META.createdAt,
    };
    const inMemory = new ByteWriter();
    await mergeResultFilesToParquet({ files, meta, writer: inMemory });
    const chunks: Uint8Array[] = [];
    let inFlight = 0;
    let maxInFlight = 0;
    const streamed = parquetStreamWriter(async (chunk) => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      await Promise.resolve();
      chunks.push(chunk);
      inFlight -= 1;
    }, 1024);

    await mergeResultFilesToParquet({ files, meta, writer: streamed });

    expect(Buffer.concat(chunks)).toEqual(Buffer.from(inMemory.getBuffer()));
    expect(chunks.length).toBeGreaterThan(3);
    expect(maxInFlight).toBe(1);
    expect(Math.max(...chunks.map((chunk) => chunk.byteLength))).toBeLessThan(
      64 * 1024
    );
    expect(() => streamed.getBytes()).toThrow("does not retain");
  });
});
