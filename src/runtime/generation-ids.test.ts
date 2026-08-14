import { describe, expect, it } from "bun:test";

import { all, flatMap, forEach, runPromise } from "effect/Effect";

import {
  getCollectedGenerationIdEntries,
  getCollectedGenerationIds,
  recordGenerationId,
  resetGenerationIds,
  withAuxiliaryUsage,
} from "./generation-ids";
describe("generation id collector", () => {
  it("flags cache-hit ids in collected entries", async () => {
    const entries = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-real")),
        flatMap(() => recordGenerationId("gen-dummy", true)),
        flatMap(() => recordGenerationId("gen-source", true, true)),
        flatMap(() => getCollectedGenerationIdEntries)
      )
    );
    const sorted = [...entries].toSorted((a, b) => a.id.localeCompare(b.id));
    expect(sorted).toEqual([
      {
        id: "gen-dummy",
        isCacheHit: true,
        countsTowardUsage: true,
        isResolvedSource: false,
      },
      {
        id: "gen-real",
        isCacheHit: false,
        countsTowardUsage: true,
        isResolvedSource: false,
      },
      {
        id: "gen-source",
        isCacheHit: true,
        countsTowardUsage: true,
        isResolvedSource: true,
      },
    ]);
  });
  it("marks ids recorded inside withAuxiliaryUsage as not counting toward usage", async () => {
    const entries = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-solver", true)),
        flatMap(() =>
          withAuxiliaryUsage(recordGenerationId("gen-judge", true))
        ),
        flatMap(() => getCollectedGenerationIdEntries)
      )
    );
    const sorted = [...entries].toSorted((a, b) => a.id.localeCompare(b.id));
    expect(sorted).toEqual([
      {
        id: "gen-judge",
        isCacheHit: true,
        countsTowardUsage: false,
        isResolvedSource: false,
      },
      {
        id: "gen-solver",
        isCacheHit: true,
        countsTowardUsage: true,
        isResolvedSource: false,
      },
    ]);
  });
  it("records, ignores empty ids, and resets", async () => {
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() => recordGenerationId("gen-1")),
        flatMap(() => recordGenerationId("")),
        flatMap(() => recordGenerationId(null)),
        flatMap(() => getCollectedGenerationIds)
      )
    );
    expect(ids).toEqual(["gen-1"]);
    expect(
      await runPromise(
        resetGenerationIds.pipe(flatMap(() => getCollectedGenerationIds))
      )
    ).toEqual([]);
  });
  it("isolates ids recorded by concurrent fibers", async () => {
    const ids = await runPromise(
      all(
        [
          resetGenerationIds.pipe(
            flatMap(() => recordGenerationId("left")),
            flatMap(() => getCollectedGenerationIds)
          ),
          resetGenerationIds.pipe(
            flatMap(() => recordGenerationId("right")),
            flatMap(() => getCollectedGenerationIds)
          ),
        ],
        { concurrency: 2 }
      )
    );
    expect(ids).toEqual([["left"], ["right"]]);
  });
  it("merges ids recorded by concurrent child fibers back into the parent", async () => {
    const ids = await runPromise(
      resetGenerationIds.pipe(
        flatMap(() =>
          forEach(["gen-a", "gen-b", "gen-c", "gen-d"], recordGenerationId, {
            concurrency: 4,
          })
        ),
        flatMap(() => getCollectedGenerationIds)
      )
    );
    expect([...ids].toSorted()).toEqual(["gen-a", "gen-b", "gen-c", "gen-d"]);
  });
});
