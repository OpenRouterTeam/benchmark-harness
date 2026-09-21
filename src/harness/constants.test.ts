import { describe, expect, it } from "bun:test";

import { defaultReasoningEffortFor, reasoningRequestFor } from "./constants";

describe("reasoning effort constants", () => {
  it("defaults openrouter/jev to auto and everything else to high", () => {
    expect(defaultReasoningEffortFor("openrouter/jev")).toBe("auto");
    expect(defaultReasoningEffortFor("openrouter/jev:nitro")).toBe("auto");
    expect(defaultReasoningEffortFor("openai/gpt-5")).toBe("high");
    expect(defaultReasoningEffortFor(undefined)).toBe("high");
  });

  it("maps auto to no reasoning object and pins every other effort", () => {
    expect(reasoningRequestFor("auto")).toBeUndefined();
    expect(reasoningRequestFor("high")).toEqual({ effort: "high" });
    expect(reasoningRequestFor("none")).toEqual({ effort: "none" });
  });
});
