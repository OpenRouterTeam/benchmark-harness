import { describe, expect, it } from "bun:test";

import { deterministicIntegrity, judgedIntegrity } from "./integrity-spec";

describe("deterministicIntegrity", () => {
  it("flags empty completions and passes bare answers", () => {
    expect(deterministicIntegrity(" \n")).toEqual({
      source: "empty",
      notIntact: 1,
      flagged: true,
    });
    expect(deterministicIntegrity("Answer: C")).toEqual({
      source: "bare_answer",
      notIntact: 0,
      flagged: false,
    });
  });

  it("leaves other completions to the judge", () => {
    expect(deterministicIntegrity("Because X.\n\nAnswer: C")).toBeUndefined();
  });
});

describe("judgedIntegrity", () => {
  it("flags when not-intact probability exceeds the threshold", () => {
    expect(judgedIntegrity({ coherent_good: 0.59 })?.flagged).toBe(true);
    expect(judgedIntegrity({ coherent_good: 0.6 })?.flagged).toBe(false);
  });

  it("is undefined without a coherent_good probability", () => {
    expect(judgedIntegrity({ coherent_bad: 1 })).toBeUndefined();
  });
});
