import { describe, expect, it } from "bun:test";

import { detectContamination } from "./contamination";
describe("detectContamination", () => {
  it("flags leak markers in the content (case-insensitive)", () => {
    const signals = detectContamination({
      content: "see perplexity-ai/draco for details",
      fusionAnalysis: null,
    });
    expect(signals).toContain("leak-marker:perplexity-ai/draco");
  });
  it("scans the serialized fusionAnalysis too", () => {
    const signals = detectContamination({
      content: "clean answer",
      fusionAnalysis: { unique_insights: [{ insight: "pplx-draco found" }] },
    });
    expect(signals).toContain("leak-marker:pplx-draco");
  });
  it("flags benchmark self-recognition phrasing", () => {
    const signals = detectContamination({
      content:
        "This prompt appears to be a benchmark task designed to test me.",
      fusionAnalysis: null,
    });
    expect(signals).toContain("benchmark-self-recognition");
  });
  it('does NOT flag incidental mentions of "benchmark" as the subject', () => {
    const signals = detectContamination({
      content: "The MMLU benchmark is a common eval suite for language models.",
      fusionAnalysis: null,
    });
    expect(signals).not.toContain("benchmark-self-recognition");
  });
  it("returns empty for a clean generation", () => {
    expect(
      detectContamination({
        content: "A factual research answer.",
        fusionAnalysis: null,
      })
    ).toEqual([]);
  });
  it("returns empty for empty content and no fusion analysis", () => {
    expect(
      detectContamination({ content: null, fusionAnalysis: null })
    ).toEqual([]);
  });
});
