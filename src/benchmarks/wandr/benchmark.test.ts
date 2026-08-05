import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { WandrConfigSchema } from "../benchmark-config";
import { wandrInferenceOverride } from "./benchmark";
describe("WANDR benchmark configuration", () => {
  it("forwards costTier into the solver inference override", () => {
    const parsed = parseSchema(WandrConfigSchema, {
      benchmarkId: "wandr",
      model: "openai/gpt-5.4",
      costTier: "high",
    });
    assertRight(parsed);
    expect(wandrInferenceOverride(parsed.right).costTier).toBe("high");
  });
});
