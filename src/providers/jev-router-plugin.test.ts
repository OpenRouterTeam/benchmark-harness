import { describe, expect, it } from "bun:test";

import { buildJevRouterPlugin } from "./jev-router-plugin";

describe("buildJevRouterPlugin", () => {
  it.each(["low", "medium", "high"] as const)(
    "forwards the %s tier",
    (tier) => {
      expect(buildJevRouterPlugin("typesafe/jev-router", tier)).toEqual({
        id: "jev-router",
        cost_tier: tier,
      });
    }
  );

  it("preserves the API default when no tier is supplied", () => {
    expect(
      buildJevRouterPlugin("typesafe/jev-router", undefined)
    ).toBeUndefined();
  });

  it.each([
    undefined,
    "openrouter/auto",
    "openrouter/auto-beta",
    "openai/gpt-5",
  ])("does not add a Jev plugin to %s", (model) => {
    expect(buildJevRouterPlugin(model, "high")).toBeUndefined();
  });
});
