import { describe, expect, it } from "bun:test";

import { MessageRole, ScoreValue } from "../harness/core";
import type { SampleOutcome } from "../harness/run";
import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import type { PartialOutcomesPayload } from "./partial-outcome-store";
import {
  isSameRunScope,
  PartialOutcomesPayloadSchema,
} from "./partial-outcome-store";

const OUTCOME: SampleOutcome = {
  sampleScore: {
    sampleId: "s-1",
    epoch: 0,
    score: {
      value: ScoreValue.Correct,
      answer: "B",
      explanation: "matched target",
      trajectory: { kind: "verifier_log", log: "verified B" },
    },
    messages: [{ role: MessageRole.Assistant, content: "Answer: B" }],
    responseItems: [{ type: "message", id: "resp-1" }],
    requestBody: { model: "test-model", temperature: 0 },
    generationIds: ["gen-1"],
    metadata: { category: "math" },
    input: "Q1 target B",
    target: "B",
  },
  usage: {
    inputTokens: 10,
    outputTokens: 5,
    totalTokens: 15,
    reasoningTokens: 2,
    totalCost: 0.001,
    serverToolUse: {
      webSearchRequests: 1,
      toolCallsRequested: 2,
      toolCallsExecuted: 2,
    },
  },
  generationTimeMs: 100,
};

const PAYLOAD: PartialOutcomesPayload = {
  scope: { epochs: 2, range: { start: 0, end: 10 } },
  outcomes: [OUTCOME],
};

describe("PartialOutcomesPayloadSchema", () => {
  it("round-trips a JSON-serialized payload back to an equal value", () => {
    const wire: unknown = JSON.parse(JSON.stringify(PAYLOAD));

    const parsed = parseSchema(PartialOutcomesPayloadSchema, wire);

    assertRight(parsed);
    expect(parsed.right).toEqual(PAYLOAD);
  });

  it("round-trips a payload with only required fields", () => {
    const minimal: PartialOutcomesPayload = {
      scope: { epochs: 1 },
      outcomes: [
        {
          sampleScore: {
            sampleId: "s-2",
            epoch: 1,
            score: {
              value: ScoreValue.Skipped,
              answer: null,
              explanation: "interrupted",
            },
          },
        },
      ],
    };
    const wire: unknown = JSON.parse(JSON.stringify(minimal));

    const parsed = parseSchema(PartialOutcomesPayloadSchema, wire);

    assertRight(parsed);
    expect(parsed.right).toEqual(minimal);
  });

  it("rejects an outcome with an unknown score value", () => {
    const wire: unknown = JSON.parse(
      JSON.stringify({
        ...PAYLOAD,
        outcomes: [
          {
            ...OUTCOME,
            sampleScore: {
              ...OUTCOME.sampleScore,
              score: { ...OUTCOME.sampleScore.score, value: "X" },
            },
          },
        ],
      })
    );

    const parsed = parseSchema(PartialOutcomesPayloadSchema, wire);

    assertLeft(parsed);
  });

  it("rejects a payload without a run scope", () => {
    const wire: unknown = JSON.parse(
      JSON.stringify({ outcomes: PAYLOAD.outcomes })
    );

    const parsed = parseSchema(PartialOutcomesPayloadSchema, wire);

    assertLeft(parsed);
  });
});

describe("isSameRunScope", () => {
  it("matches identical scopes with and without ranges", () => {
    expect(isSameRunScope({ epochs: 2 }, { epochs: 2 })).toBe(true);
    expect(
      isSameRunScope(
        { epochs: 2, range: { start: 0, end: 10 } },
        { epochs: 2, range: { start: 0, end: 10 } }
      )
    ).toBe(true);
  });

  it("rejects scopes with different epochs", () => {
    expect(isSameRunScope({ epochs: 2 }, { epochs: 3 })).toBe(false);
  });

  it("rejects scopes with different or missing ranges", () => {
    expect(
      isSameRunScope(
        { epochs: 2, range: { start: 0, end: 10 } },
        { epochs: 2, range: { start: 0, end: 5 } }
      )
    ).toBe(false);
    expect(
      isSameRunScope({ epochs: 2, range: { start: 0, end: 10 } }, { epochs: 2 })
    ).toBe(false);
  });
});
