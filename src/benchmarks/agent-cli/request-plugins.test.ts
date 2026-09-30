import { describe, expect, it } from "bun:test";

import { getOriHarness } from "./harness";
import { sandboxAgentRequestPlugins } from "./request-plugins";

const BASE = {
  benchmarkId: "terminal_bench",
  model: "openrouter/auto",
  costTier: undefined,
  costQualityTradeoff: undefined,
  pinModel: undefined,
} as const;

describe("sandboxAgentRequestPlugins", () => {
  it("builds the auto-router plugin in wire form for pi", () => {
    for (const costTier of ["low", "medium", "high"] as const) {
      expect(
        sandboxAgentRequestPlugins({
          ...BASE,
          harness: getOriHarness("pi"),
          costTier,
        })
      ).toEqual([{ id: "auto-router", cost_tier: costTier }]);
    }
  });

  it("carries every auto-router option and ignores the model variant suffix", () => {
    expect(
      sandboxAgentRequestPlugins({
        ...BASE,
        harness: getOriHarness("pi"),
        model: "openrouter/auto-beta:nitro",
        costTier: "high",
        costQualityTradeoff: 3,
        pinModel: true,
      })
    ).toEqual([
      {
        id: "auto-beta-router",
        cost_tier: "high",
        cost_quality_tradeoff: 3,
        pin_model: true,
      },
    ]);
  });

  it("returns no plugins without auto-router options or for a concrete model", () => {
    expect(
      sandboxAgentRequestPlugins({ ...BASE, harness: getOriHarness("claude") })
    ).toEqual([]);
    expect(
      sandboxAgentRequestPlugins({
        ...BASE,
        harness: getOriHarness("claude"),
        model: "anthropic/claude-sonnet-4.5",
        costTier: "low",
      })
    ).toEqual([]);
  });

  it("rejects auto-router options for agents that cannot forward the plugin", () => {
    for (const agent of ["claude", "prime-agent", "omp"] as const) {
      const result = sandboxAgentRequestPlugins({
        ...BASE,
        harness: getOriHarness(agent),
        costTier: "low",
      });
      expect(result).toBeInstanceOf(Error);
      expect(result instanceof Error ? result.message : "").toBe(
        `terminal_bench cannot use costTier: the ${agent} agent calls the model from inside the sandbox and does not forward the auto-router plugin (supported agents: pi)`
      );
    }
  });

  it("names every rejected auto-router option", () => {
    const result = sandboxAgentRequestPlugins({
      ...BASE,
      harness: getOriHarness("omp"),
      costQualityTradeoff: 5,
      pinModel: true,
    });
    expect(result instanceof Error ? result.message : "").toStartWith(
      "terminal_bench cannot use costQualityTradeoff, pinModel: the omp agent"
    );
  });
});
