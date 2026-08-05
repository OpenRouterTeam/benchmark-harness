import { describe, expect, it } from "bun:test";

import { Either } from "../../internal/either";
import { assertLeft, assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BenchmarkRunConfigSchema } from "../benchmark-config";
import { renderPrompt } from "./benchmark";
import { inlineCaseToSample } from "./dataset";
import { extractLastNumber, scoreCompletion } from "./scorer";
import { EvalSpecSchema } from "./spec";

const spec = (overrides: Record<string, unknown> = {}): unknown => ({
  name: "my eval",
  dataset: {
    kind: "inline",
    cases: [{ input: "What is 2+2?", target: "4" }],
  },
  scorer: { kind: "exact" },
  ...overrides,
});

describe("EvalSpecSchema", () => {
  it("accepts a minimal inline spec and applies defaults", () => {
    const parsed = parseSchema(EvalSpecSchema, spec());
    expect(Either.isRight(parsed)).toBe(true);
    if (Either.isRight(parsed)) {
      expect(parsed.right.specVersion).toBe(1);
      expect(parsed.right.scorer).toMatchObject({
        kind: "exact",
        caseSensitive: false,
        trim: true,
      });
    }
  });

  it("accepts an HF dataset spec with field mapping", () => {
    const parsed = parseSchema(
      EvalSpecSchema,
      spec({
        dataset: {
          kind: "hf",
          dataset: "openai/gsm8k",
          split: "test",
          inputField: "question",
          targetField: "answer",
        },
        scorer: { kind: "numeric", absoluteTolerance: 0 },
      })
    );
    expect(Either.isRight(parsed)).toBe(true);
  });

  it("rejects empty inline datasets and unknown scorers", () => {
    expect(
      Either.isLeft(
        parseSchema(
          EvalSpecSchema,
          spec({ dataset: { kind: "inline", cases: [] } })
        )
      )
    ).toBe(true);
    expect(
      Either.isLeft(
        parseSchema(EvalSpecSchema, spec({ scorer: { kind: "vibes" } }))
      )
    ).toBe(true);
  });

  it("round-trips through the benchmark config union", () => {
    const parsed = parseSchema(BenchmarkRunConfigSchema, {
      benchmarkId: "custom_eval",
      model: "openai/gpt-4o",
      spec: spec(),
    });
    expect(Either.isRight(parsed)).toBe(true);
    if (Either.isRight(parsed) && parsed.right.benchmarkId === "custom_eval") {
      expect(parsed.right.spec.name).toBe("my eval");
    }
  });
});

describe("scoreCompletion", () => {
  it("exact: trims and case-folds by default", () => {
    const scorer = { kind: "exact", caseSensitive: false, trim: true } as const;
    expect(scoreCompletion(scorer, "  Paris \n", "paris").value).toBe("C");
    expect(scoreCompletion(scorer, "London", "paris").value).toBe("I");
  });

  it("contains: substring match", () => {
    const scorer = { kind: "contains", caseSensitive: false } as const;
    expect(
      scoreCompletion(scorer, "The answer is Paris, France.", "paris").value
    ).toBe("C");
    expect(scoreCompletion(scorer, "No idea.", "paris").value).toBe("I");
  });

  it("regex: pattern match with case-insensitive default", () => {
    const scorer = {
      kind: "regex",
      pattern: "answer:\\s*42",
      caseSensitive: false,
    } as const;
    expect(scoreCompletion(scorer, "ANSWER: 42", "unused").value).toBe("C");
    expect(scoreCompletion(scorer, "answer: 41", "unused").value).toBe("I");
  });

  it("choice: reuses MCQ letter extraction", () => {
    const scorer = { kind: "choice" } as const;
    expect(scoreCompletion(scorer, "Thinking...\nAnswer: B", "b").value).toBe(
      "C"
    );
    expect(scoreCompletion(scorer, "Answer: C", "B").value).toBe("I");
  });

  it("numeric: exact and tolerant comparison", () => {
    const exact = {
      kind: "numeric",
      absoluteTolerance: 0,
      relativeTolerance: 0,
    } as const;
    expect(scoreCompletion(exact, "The total is 1,234.", "1234").value).toBe(
      "C"
    );
    expect(scoreCompletion(exact, "roughly 1233", "1234").value).toBe("I");
    const tolerant = {
      kind: "numeric",
      absoluteTolerance: 2,
      relativeTolerance: 0,
    } as const;
    expect(scoreCompletion(tolerant, "roughly 1233", "1234").value).toBe("C");
    expect(scoreCompletion(exact, "no numbers here", "1").value).toBe("I");
  });
});

describe("extractLastNumber", () => {
  it("takes the final number, ignoring commas and signs", () => {
    expect(extractLastNumber("First 3 then 4.5, answer -7")).toBe(-7);
    expect(extractLastNumber("total: 1,234,567")).toBe(1_234_567);
    expect(extractLastNumber("none")).toBeNull();
  });
});

describe("renderPrompt / inlineCaseToSample", () => {
  it("substitutes {input} and preserves literal $ in inputs", () => {
    expect(renderPrompt("Q: {input}\nA:", "cost is $5")).toBe(
      "Q: cost is $5\nA:"
    );
    expect(renderPrompt(undefined, "raw")).toBe("raw");
  });

  it("assigns stable ids to inline cases", () => {
    expect(inlineCaseToSample({ input: "a", target: "b" }, 3).id).toBe(
      "custom_eval-3"
    );
    expect(
      inlineCaseToSample({ id: "mine", input: "a", target: "b" }, 3).id
    ).toBe("mine");
  });
});

describe("EvalScorerSchema regex validation", () => {
  it("rejects a malformed regex pattern at parse time, not scoring time", () => {
    const parsed = parseSchema(EvalSpecSchema, {
      specVersion: 1,
      name: "bad regex",
      dataset: { kind: "inline", cases: [{ input: "x", target: "y" }] },
      scorer: { kind: "regex", pattern: "[unclosed" },
    });
    assertLeft(parsed);
  });

  it("accepts a valid pattern", () => {
    const parsed = parseSchema(EvalSpecSchema, {
      specVersion: 1,
      name: "good regex",
      dataset: { kind: "inline", cases: [{ input: "x", target: "y" }] },
      scorer: { kind: "regex", pattern: String.raw`\d{4}-\d{2}-\d{2}` },
    });
    assertRight(parsed);
  });
});
