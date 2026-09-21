import { describe, expect, it } from "bun:test";

import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import {
  DecisionsAnswerSchema,
  DecisionsQuestionSchema,
  DecisionsRequestSchema,
  DecisionsResponseSchema,
} from "./schema";

function accepts<Output>(
  schema: Parameters<typeof parseSchema<Output>>[0],
  value: unknown
): boolean {
  return Either.isRight(parseSchema(schema, value));
}

describe("DecisionsQuestionSchema", () => {
  it("accepts literal and structured instructions and criteria", () => {
    expect(
      accepts(DecisionsQuestionSchema, { type: "noul", instructions: "Is it?" })
    ).toBe(true);
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "noul",
        instructions: { goal: "detect greeting" },
        criteria: { true: ["hi", "hello"], false: { note: "anything else" } },
      })
    ).toBe(true);
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "choice",
        instructions: "Pick one",
        criteria: { a: "first", b: null, c: { detail: ["x"] } },
      })
    ).toBe(true);
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "score",
        instructions: "Rate",
        criteria: ["bad", { level: "ok" }, ["great"]],
      })
    ).toBe(true);
  });
  it("rejects wrong criteria shapes and unknown types", () => {
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "noul",
        instructions: "Is it?",
        criteria: { yes: "a", no: "b" },
      })
    ).toBe(false);
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "choice",
        instructions: "Pick",
        criteria: ["a", "b"],
      })
    ).toBe(false);
    expect(
      accepts(DecisionsQuestionSchema, {
        type: "score",
        instructions: "Rate",
        criteria: { low: "a" },
      })
    ).toBe(false);
    expect(
      accepts(DecisionsQuestionSchema, { type: "rank", instructions: "x" })
    ).toBe(false);
    expect(
      accepts(DecisionsQuestionSchema, { type: "noul", instructions: 3 })
    ).toBe(false);
  });
});

describe("DecisionsRequestSchema", () => {
  it("accepts string, object, and array state with optional provider routing", () => {
    for (const state of ["text", { a: 1 }, [1, "two"]]) {
      expect(
        accepts(DecisionsRequestSchema, {
          model: "typesafe/jev-latest",
          state,
          questions: { q: { type: "noul", instructions: "?" } },
        })
      ).toBe(true);
    }
    expect(
      accepts(DecisionsRequestSchema, {
        model: "typesafe/jev-1.13.0",
        state: "s",
        questions: {},
        provider: { only: ["typesafe"], allow_fallbacks: false },
      })
    ).toBe(true);
  });
  it("rejects a missing model, missing questions, or malformed provider", () => {
    expect(
      accepts(DecisionsRequestSchema, {
        state: "s",
        questions: { q: { type: "noul", instructions: "?" } },
      })
    ).toBe(false);
    expect(accepts(DecisionsRequestSchema, { model: "m", state: "s" })).toBe(
      false
    );
    expect(
      accepts(DecisionsRequestSchema, {
        model: "m",
        state: "s",
        questions: {},
        provider: { only: "typesafe" },
      })
    ).toBe(false);
  });
});

describe("DecisionsAnswerSchema", () => {
  it("accepts each answer type with optional confidence and preserves extra fields", () => {
    expect(accepts(DecisionsAnswerSchema, { type: "noul", noul: 0.5 })).toBe(
      true
    );
    const choice = parseSchema(DecisionsAnswerSchema, {
      type: "choice",
      choice: "a",
      probabilities: { a: 0.7, b: 0.3 },
      extra: "kept",
    });
    expect(Either.isRight(choice)).toBe(true);
    expect(Either.isRight(choice) ? choice.right : undefined).toMatchObject({
      extra: "kept",
    });
    expect(
      accepts(DecisionsAnswerSchema, {
        type: "score",
        score: 1.5,
        legend: { "0": "bad", "1": "ok", "2": "good" },
        probabilities: { "0": 0, "1": 0.5, "2": 0.5 },
        confidence: 0.5,
      })
    ).toBe(true);
  });
  it("rejects wrong value types and missing required fields", () => {
    expect(accepts(DecisionsAnswerSchema, { type: "noul", noul: "0.5" })).toBe(
      false
    );
    expect(
      accepts(DecisionsAnswerSchema, { type: "choice", choice: "a" })
    ).toBe(false);
    expect(
      accepts(DecisionsAnswerSchema, {
        type: "choice",
        choice: "a",
        probabilities: { a: "1" },
      })
    ).toBe(false);
    expect(
      accepts(DecisionsAnswerSchema, {
        type: "score",
        score: 1,
        probabilities: { "0": 1 },
      })
    ).toBe(false);
  });
});

describe("DecisionsResponseSchema", () => {
  it("accepts a minimal response and a full response", () => {
    expect(accepts(DecisionsResponseSchema, { model: "m", answers: {} })).toBe(
      true
    );
    const full = parseSchema(DecisionsResponseSchema, {
      id: "gen-1",
      model: "typesafe/jev-1.13.0",
      provider: "typesafe",
      answers: {
        "weird id / 1": { type: "noul", noul: 1 },
      },
      usage: { input_tokens: 5, output_tokens: 0, cost: 0.0000002, latency: 3 },
      created: 1,
    });
    expect(Either.isRight(full)).toBe(true);
    const response = Either.isRight(full) ? full.right : undefined;
    expect(Object.keys(response?.answers ?? {})).toEqual(["weird id / 1"]);
    expect(response?.usage?.cost).toBeCloseTo(0.0000002, 12);
    expect(response).toMatchObject({ created: 1 });
  });
  it("rejects missing model, malformed answers, and malformed usage", () => {
    expect(accepts(DecisionsResponseSchema, { answers: {} })).toBe(false);
    expect(
      accepts(DecisionsResponseSchema, {
        model: "m",
        answers: { q: { type: "noul" } },
      })
    ).toBe(false);
    expect(
      accepts(DecisionsResponseSchema, {
        model: "m",
        answers: {},
        usage: { input_tokens: "5", output_tokens: 0 },
      })
    ).toBe(false);
    expect(
      accepts(DecisionsResponseSchema, {
        model: "m",
        answers: {},
        usage: { input_tokens: 5 },
      })
    ).toBe(false);
  });
});
