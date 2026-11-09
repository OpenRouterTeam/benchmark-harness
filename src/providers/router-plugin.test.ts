import { describe, expect, it } from "bun:test";

import { buildRouterPlugin } from "./router-plugin";

describe.each([
  ["openrouter/auto", "auto-router"],
  ["openrouter/auto-beta", "auto-beta-router"],
  ["typesafe/jev-router", "jev-router"],
] as const)("buildRouterPlugin: %s", (model, pluginId) => {
  it.each(["low", "medium", "high"] as const)(
    "forwards %s in wire form",
    (tier) => {
      expect(buildRouterPlugin(model, { costTier: tier })).toEqual({
        id: pluginId,
        cost_tier: tier,
      });
      expect(buildRouterPlugin(`${model}:nitro`, { costTier: tier })).toEqual({
        id: pluginId,
        cost_tier: tier,
      });
    }
  );

  it("preserves the default when no controls are supplied", () => {
    expect(buildRouterPlugin(model, {})).toBeUndefined();
    expect(buildRouterPlugin(model, { pinModel: false })).toBeUndefined();
  });

  it("only includes Auto Router controls for Auto Router", () => {
    const options = { costQualityTradeoff: 0, pinModel: true } as const;
    expect(buildRouterPlugin(model, options)).toEqual(
      pluginId === "jev-router"
        ? undefined
        : { id: pluginId, cost_quality_tradeoff: 0, pin_model: true }
    );
    expect(buildRouterPlugin(model, { ...options, costTier: "high" })).toEqual(
      pluginId === "jev-router"
        ? { id: pluginId, cost_tier: "high" }
        : {
            id: pluginId,
            cost_tier: "high",
            cost_quality_tradeoff: 0,
            pin_model: true,
          }
    );
  });
});

describe("buildRouterPlugin", () => {
  it.each([undefined, "openai/gpt-5"])(
    "does not add routing controls to %s",
    (model) => {
      expect(
        buildRouterPlugin(model, {
          costTier: "high",
          costQualityTradeoff: 8,
          pinModel: true,
          switchyardAlgorithm: "stage",
        })
      ).toBeUndefined();
    }
  );

  it("preserves Switchyard's separate algorithm control", () => {
    expect(
      buildRouterPlugin("nvidia/switchyard:nitro", {
        switchyardAlgorithm: "stage",
        costTier: "high",
        pinModel: true,
      })
    ).toEqual({ id: "switchyard-router", algorithm: "stage" });
    expect(
      buildRouterPlugin("nvidia/switchyard", { costTier: "high" })
    ).toBeUndefined();
  });
});
