import { describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { AsyncBuffer } from "hyparquet";

import type { BenchmarkMetadata } from "../benchmarks/types";
import { MessageRole, ScoreValue } from "../harness/core";
import type { RunResult } from "../harness/run";
import { runHarnessPromise } from "../internal/effect-logger";
import { readResultRows } from "./parquet";
import { makeLocalResultStore } from "./result-store";

const PNG = Buffer.from("png-bytes").toString("base64");

const RESULT: RunResult = {
  metrics: {
    accuracy: 1,
    totalQuestions: 1,
    correctAnswers: 1,
    skippedQuestions: 0,
  },
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    totalTokens: 0,
    reasoningTokens: 0,
    totalCost: 0,
    generationTimeMs: 0,
  },
  sampleScores: [
    {
      sampleId: "mmmu-1",
      epoch: 0,
      score: { value: ScoreValue.Correct, answer: "A", explanation: "" },
      messages: [
        {
          role: MessageRole.User,
          content: "Which organ?",
          contentParts: [
            {
              type: "image_url",
              imageUrl: { url: `data:image/png;base64,${PNG}` },
            },
          ],
        },
        { role: MessageRole.Assistant, content: "A" },
      ],
    },
  ],
};

const BENCHMARK: BenchmarkMetadata = {
  id: "mmmu",
  temperature: 0,
  defaultEpochs: 1,
  makeDatasetLayer: () => {
    throw new Error("unused");
  },
};

async function writeAndRead(
  store: ReturnType<typeof makeLocalResultStore>
): Promise<Readonly<Record<string, unknown>>> {
  const path = await runHarnessPromise(
    store.write({
      result: RESULT,
      benchmark: BENCHMARK,
      benchmarkConfig: { benchmarkId: "mmmu", model: "openai/gpt-4o-mini" },
      epochs: 1,
      sessionId: "session",
    })
  );
  const bytes = readFileSync(path ?? "");
  const file: AsyncBuffer = {
    byteLength: bytes.byteLength,
    slice: (start, end) =>
      bytes.buffer.slice(
        bytes.byteOffset + start,
        bytes.byteOffset + (end ?? bytes.byteLength)
      ),
  };
  const [row] = await readResultRows(file);
  return row ?? {};
}

describe("makeLocalResultStore", () => {
  it("writes artifact pointers into the trajectory and messages columns", async () => {
    const uploads: string[] = [];
    const row = await writeAndRead(
      makeLocalResultStore({
        dir: mkdtempSync(join(tmpdir(), "result-store-")),
        artifactSink: {
          uriFor: (path) => `gs://results/run-1/${path}`,
          put: (artifact) => {
            uploads.push(artifact.path);
            return Promise.resolve();
          },
        },
      })
    );

    const [path] = uploads;
    const uri = `gs://results/run-1/${path}`;
    expect(uploads).toHaveLength(1);
    expect(JSON.stringify(row)).not.toContain(PNG);
    expect(JSON.parse(String(row.messages))[0].content_parts).toEqual([
      { type: "image_url", image_url: { url: uri } },
    ]);
    expect(JSON.parse(String(row.trajectory)).steps[0].message).toContainEqual({
      type: "image",
      source: { media_type: "image/png", path: uri },
    });
  });

  it("keeps inline images when no artifact sink is configured", async () => {
    const row = await writeAndRead(
      makeLocalResultStore({
        dir: mkdtempSync(join(tmpdir(), "result-store-")),
      })
    );

    expect(JSON.parse(String(row.messages))[0].content_parts).toEqual([
      { type: "image_url", image_url: { url: `data:image/png;base64,${PNG}` } },
    ]);
  });
});
