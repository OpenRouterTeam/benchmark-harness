import { describe, expect, it } from "bun:test";

import { MessageRole, ScoreValue } from "../harness/core";
import type { SampleOutcome } from "../harness/run";
import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { PartialOutcomesSchema } from "./partial-outcome-store";

const OUTCOME: SampleOutcome = {
  sampleScore: {
    sampleId: "s-1",
    epoch: 0,
    score: {
      value: ScoreValue.Correct,
      answer: "B",
      explanation: "matched target",
    },
    messages: [{ role: MessageRole.Assistant, content: "Answer: B" }],
    generationIds: ["gen-1"],
  },
  usage: {
    inputTokens: 10,
    outputTokens: 5,
    totalTokens: 15,
    totalCost: 0.001,
  },
  generationTimeMs: 100,
};

describe("PartialOutcomesSchema", () => {
  it("round-trips a JSON-serialized outcome back to an equal SampleOutcome", () => {
    const wire: unknown = JSON.parse(JSON.stringify([OUTCOME]));

    const parsed = parseSchema(PartialOutcomesSchema, wire);

    assertRight(parsed);
    expect(parsed.right).toEqual([OUTCOME]);
  });

  it("rejects an outcome with an unknown score value", () => {
    const wire: unknown = JSON.parse(
      JSON.stringify([
        {
          ...OUTCOME,
          sampleScore: {
            ...OUTCOME.sampleScore,
            score: { ...OUTCOME.sampleScore.score, value: "X" },
          },
        },
      ])
    );

    const parsed = parseSchema(PartialOutcomesSchema, wire);

    assertLeft(parsed);
  });
});
