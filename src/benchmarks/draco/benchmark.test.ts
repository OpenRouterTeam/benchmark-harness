import { describe, expect, it } from "bun:test";
import { readFile } from "node:fs/promises";

import { FetchHttpClient } from "@effect/platform";
import { gen, provide, runPromise } from "effect/Effect";
import { provide as layerProvide, succeed } from "effect/Layer";

import { ScoreValue } from "../../harness/core";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../harness/progress";
import type { RunResult } from "../../harness/run";
import { Solver } from "../../harness/solver";
import { DRACO_BENCHMARK } from "./benchmark";

function runResultWith(
  explanations: readonly (string | undefined)[]
): RunResult {
  return {
    metrics: {
      accuracy: 0,
      totalQuestions: explanations.length,
      correctAnswers: 0,
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
    sampleScores: explanations.map((explanation, i) => ({
      sampleId: `s${i}`,
      epoch: 0,
      score: {
        value: ScoreValue.Correct,
        answer: "",
        explanation: explanation ?? "",
      },
    })),
  };
}
describe("DRACO runLevelScores", () => {
  it("aggregates normalized + passRate means from per-sample explanations", () => {
    const result = runResultWith([
      JSON.stringify({ normalized: 60, passRate: 70 }),
      JSON.stringify({ normalized: 80, passRate: 90 }),
    ]);
    const scores = DRACO_BENCHMARK.runLevelScores?.(result) ?? [];
    expect(scores).toHaveLength(1);
    expect(scores[0]?.name).toBe("draco");
    expect(scores[0]?.metrics).toEqual({
      normalized: { value: 70 },
      pass_rate: { value: 80 },
      samples_scored: { value: 2 },
    });
  });
  it("skips samples with missing or unparsable explanations", () => {
    const result = runResultWith([
      JSON.stringify({ normalized: 50, passRate: 60 }),
      undefined,
      "not json",
      JSON.stringify({ normalized: 70, passRate: 80 }),
    ]);
    const scores = DRACO_BENCHMARK.runLevelScores?.(result) ?? [];
    expect(scores[0]?.metrics.normalized).toEqual({ value: 60 });
    expect(scores[0]?.metrics.pass_rate).toEqual({ value: 70 });
    expect(scores[0]?.metrics.samples_scored).toEqual({ value: 2 });
  });
  it("returns no extra scores when no sample has a parsable explanation", () => {
    const result = runResultWith([undefined, "garbage"]);
    const scores = DRACO_BENCHMARK.runLevelScores?.(result) ?? [];
    expect(scores).toEqual([]);
  });
});

describe("DRACO makeLayer", () => {
  it("forwards trace headers to inference requests", async () => {
    const stream = await readFile(
      new URL(
        "../../../test/fixtures/advisor-responses-stream.sse",
        import.meta.url
      ),
      "utf8"
    );
    const originalFetch = globalThis.fetch;
    let capturedHeaders: Headers | undefined;
    globalThis.fetch = async (fetchInput, init) => {
      const request =
        fetchInput instanceof Request
          ? fetchInput
          : new Request(fetchInput, init);
      if (capturedHeaders === undefined) {
        capturedHeaders = request.headers;
      }
      return new Response(stream, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    };
    try {
      const layer = DRACO_BENCHMARK.makeLayer({
        apiKey: "sk-test",
        baseUrl: "https://example.test",
        sessionId: "session-1",
        benchmarkConfig: {
          benchmarkId: "draco",
          panelConfig: {
            name: "test-exp",
            description: "",
            type: "single",
            model: "openai/gpt-5",
            analysisModels: [],
            searchEngine: null,
            blockedDomains: [],
            judgeModel: "openai/gpt-5",
            judgeRuns: 1,
            judgeReasoningEffort: "low",
            criterionConcurrency: 1,
            timeout: 1800,
            concurrency: 1,
          },
        },
        traceHeaders: {
          traceparent:
            "00-11111111111111111111111111111111-2222222222222222-01",
          "x-benchmark-trace": "bench-key",
        },
      });
      await runPromise(
        gen(function* () {
          const solver = yield* Solver;
          yield* solver({
            sample: { id: "s1", input: "Q?", target: { text: "A" } },
            messages: [],
            completed: false,
          });
        }).pipe(
          provide(layer.pipe(layerProvide(FetchHttpClient.layer))),
          provide(succeed(ProgressReporter, NOOP_PROGRESS_REPORTER)),
          provide(succeed(CheckpointStore, NOOP_CHECKPOINT_STORE))
        )
      );
      expect(capturedHeaders?.get("traceparent")).toBe(
        "00-11111111111111111111111111111111-2222222222222222-01"
      );
      expect(capturedHeaders?.get("x-benchmark-trace")).toBe("bench-key");
      expect(capturedHeaders?.get("authorization")).toBe("Bearer sk-test");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});
