import { describe, expect, it } from "bun:test";

import * as Result from "effect/Result";

import { claimScore, coverageFromScores, parseClaimVerdict } from "./scorer";

describe("claimScore", () => {
  it("maps outcomes to MCP-Atlas weights", () => {
    expect(claimScore("fulfilled")).toBe(1);
    expect(claimScore("partially_fulfilled")).toBe(0.5);
    expect(claimScore("not_fulfilled")).toBe(0);
  });
});

describe("coverageFromScores", () => {
  it("averages claim scores", () => {
    expect(coverageFromScores([1, 0.5, 0])).toBe(0.5);
  });

  it("scores a task with no claims as zero", () => {
    expect(coverageFromScores([])).toBe(0);
  });
});

describe("parseClaimVerdict", () => {
  it("rejects non-JSON and off-schema verdicts", () => {
    expect(Result.isFailure(parseClaimVerdict("not json"))).toBe(true);
    expect(Result.isFailure(parseClaimVerdict('{"outcome": "maybe"}'))).toBe(
      true
    );
  });
});
