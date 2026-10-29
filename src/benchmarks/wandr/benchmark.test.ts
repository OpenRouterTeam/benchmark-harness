import { describe, expect, it } from "bun:test";

import { assertSuccess } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { WandrConfigSchema } from "../benchmark-config";
import { wandrInferenceOverride } from "./benchmark";
describe("WANDR benchmark configuration", () => {
  it("forwards costTier into the solver inference override", () => {
    const parsed = parseSchema(WandrConfigSchema, {
      benchmarkId: "wandr",
      model: "openai/gpt-5.4",
      reasoningEffort: "high",
      costTier: "high",
    });
    assertSuccess(parsed);
    expect(wandrInferenceOverride(parsed.success).costTier).toBe("high");
  });
  it("forwards switchyardAlgorithm into the solver inference override", () => {
    const parsed = parseSchema(WandrConfigSchema, {
      benchmarkId: "wandr",
      model: "nvidia/switchyard",
      reasoningEffort: "high",
      switchyardAlgorithm: "stage",
    });
    assertSuccess(parsed);
    expect(wandrInferenceOverride(parsed.success).switchyardAlgorithm).toBe(
      "stage"
    );
  });
});
