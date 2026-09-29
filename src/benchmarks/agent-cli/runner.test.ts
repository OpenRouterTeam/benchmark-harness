import { describe, expect, it } from "bun:test";

import { provideService, runPromise, succeed } from "effect/Effect";

import type { ReplayedUsage } from "../../runtime/generation-resolver";
import { GenerationResolver } from "../../runtime/generation-resolver";
import { resolveBilledUsage, withBilledUsage } from "./runner";

const usage = (totalCost: number): ReplayedUsage => ({
  inputTokens: 100,
  outputTokens: 10,
  totalTokens: 110,
  reasoningTokens: 5,
  totalCost,
  generationTimeMs: 200,
});

const resolveWith = (
  costs: Readonly<Record<string, number>>,
  ids: readonly string[]
): Promise<ReplayedUsage | undefined> =>
  runPromise(
    resolveBilledUsage(ids).pipe(
      provideService(GenerationResolver, {
        resolveSourceGeneration: (id) => {
          const cost = costs[id];
          return succeed(
            cost === undefined
              ? undefined
              : { sourceId: id, usage: usage(cost) }
          );
        },
      })
    )
  );

describe("resolveBilledUsage", () => {
  it("sums usage when every generation resolves", async () => {
    const billed = await resolveWith({ "gen-1": 0.01, "gen-2": 0.02 }, [
      "gen-1",
      "gen-2",
    ]);
    expect(billed?.totalCost).toBeCloseTo(0.03);
    expect(billed?.inputTokens).toBe(200);
  });

  it("returns undefined when any generation fails to resolve", async () => {
    const billed = await resolveWith({ "gen-1": 0.02 }, ["gen-1", "gen-2"]);
    expect(billed).toBeUndefined();
  });
});

describe("withBilledUsage", () => {
  it("keeps agent usage when no billed usage resolved", () => {
    expect(withBilledUsage({ totalCost: 0.04 }, undefined)).toEqual({
      totalCost: 0.04,
    });
  });

  it("replaces only the cost of agent usage", () => {
    expect(
      withBilledUsage({ inputTokens: 7, totalCost: 0 }, usage(0.05))
    ).toEqual({ inputTokens: 7, totalCost: 0.05 });
  });

  it("uses billed usage when the agent reported none", () => {
    expect(withBilledUsage(undefined, usage(0.02))).toEqual({
      inputTokens: 100,
      outputTokens: 10,
      totalTokens: 110,
      reasoningTokens: 5,
      totalCost: 0.02,
    });
  });
});
