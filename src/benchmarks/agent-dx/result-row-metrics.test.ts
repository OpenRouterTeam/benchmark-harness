import { describe, expect, it } from "bun:test";

import {
  parseVerifierVerdict,
  runTotalsFromResultParts,
  runTotalsFromResultRows,
  verdictKindFromMetadata,
} from "./result-row-metrics";

const baseRow = {
  format_version: 1,
  task: "agent_dx",
  model: "openai/gpt-5.2",
  epochs: 1,
  temperature: 0,
  benchmark_config: null,
  created_at: "2026-07-27T00:00:00Z",
  accuracy: 1,
  total_questions: 1,
  correct_answers: 1,
  input_tokens: 100,
  output_tokens: 50,
  total_tokens: 150,
  reasoning_tokens: 0,
  total_cost: 0,
  generation_time_ms: 1000,
  extra_scores: null,
  sample_id: "basic-completion",
  epoch: 0,
  input: null,
  target: null,
  score_value: "C",
  answer: null,
  explanation: "VERIFY PASS",
  messages: null,
  metadata: null,
};

describe("parseVerifierVerdict", () => {
  it("parses a structured verdict", () => {
    expect(
      parseVerifierVerdict('{"kind":"platform","detail":"gen not retrievable"}')
    ).toEqual({
      kind: "platform",
      detail: "gen not retrievable",
    });
  });

  it("degrades to undefined on missing, malformed, or off-schema content", () => {
    expect(parseVerifierVerdict("")).toBeUndefined();
    expect(parseVerifierVerdict("   \n")).toBeUndefined();
    expect(parseVerifierVerdict("not json")).toBeUndefined();
    expect(
      parseVerifierVerdict('{"kind":"meteor","detail":"x"}')
    ).toBeUndefined();
    expect(parseVerifierVerdict('{"kind":"platform"}')).toBeUndefined();
  });
});

describe("verdictKindFromMetadata", () => {
  it("reads the verdict kind off row metadata", () => {
    expect(
      verdictKindFromMetadata(JSON.stringify({ verdictKind: "fixture" }))
    ).toBe("fixture");
    expect(
      verdictKindFromMetadata(JSON.stringify({ verdictKind: "agent" }))
    ).toBe("agent");
  });

  it("returns undefined for legacy rows and invalid kinds", () => {
    expect(verdictKindFromMetadata(null)).toBeUndefined();
    expect(
      verdictKindFromMetadata(JSON.stringify({ quality: 0.8 }))
    ).toBeUndefined();
    expect(
      verdictKindFromMetadata(JSON.stringify({ verdictKind: "meteor" }))
    ).toBeUndefined();
    expect(verdictKindFromMetadata("not json")).toBeUndefined();
    expect(verdictKindFromMetadata("[1,2]")).toBeUndefined();
  });
});

describe("runTotalsFromResultParts", () => {
  it("counts every part even when parts collide on the totals tuple", () => {
    const partA = [baseRow, { ...baseRow, sample_id: "streaming-usage" }];
    const partB = [{ ...baseRow, sample_id: "tool-calling-loop" }];

    expect(runTotalsFromResultParts([partA, partB])).toEqual({
      totalCost: 0,
      totalTokens: 300,
    });
    expect(runTotalsFromResultRows([...partA, ...partB]).totalTokens).toBe(150);
  });

  it("sums distinct part totals and skips empty parts", () => {
    const partA = [{ ...baseRow, total_cost: 0.01 }];
    const partB = [
      {
        ...baseRow,
        sample_id: "tool-calling-loop",
        total_tokens: 250,
        total_cost: 0.02,
      },
    ];

    const totals = runTotalsFromResultParts([partA, [], partB]);
    expect(totals.totalTokens).toBe(400);
    expect(totals.totalCost).toBeCloseTo(0.03, 10);
  });
});
