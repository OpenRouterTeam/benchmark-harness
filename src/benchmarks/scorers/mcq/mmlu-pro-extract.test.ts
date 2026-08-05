import { describe, expect, it } from "bun:test";

import { mmluProExtractAnswer } from "./mmlu-pro-extract";
describe("mmluProExtractAnswer", () => {
  it("uses the canonical answer-is pattern first", () => {
    expect(mmluProExtractAnswer("Reasoning. The answer is (C).")).toBe("C");
    expect(mmluProExtractAnswer("The answer is J")).toBe("J");
  });
  it("uses the canonical answer-colon pattern second", () => {
    expect(mmluProExtractAnswer("Reasoning\nAnswer: H")).toBe("H");
    expect(mmluProExtractAnswer("Reasoning\nanswer: A")).toBe("A");
  });
  it("uses the final standalone option letter as the third-tier fallback", () => {
    expect(
      mmluProExtractAnswer("Options A and B were considered. Final choice: D")
    ).toBe("D");
  });
  it("removes bold markers before applying the canonical patterns", () => {
    expect(mmluProExtractAnswer("The **answer is (E)**")).toBe("E");
  });
  it("returns null when no option letter is present", () => {
    expect(mmluProExtractAnswer("No answer was provided.")).toBeNull();
  });
});
