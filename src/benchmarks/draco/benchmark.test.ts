import { describe, expect, it } from "bun:test";

import { ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
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
