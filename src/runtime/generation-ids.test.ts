import { describe, expect, it } from "bun:test";

import { all, flatMap, forEach, runPromise, yieldNow } from "effect/Effect";

import {
  getCollectedGenerationIdEntries,
  getCollectedGenerationIds,
  recordGenerationId,
  withAuxiliaryUsage,
  withFreshGenerationIds,
} from "./generation-ids";
describe("generation id collector", () => {
  it("flags cache-hit ids in collected entries", async () => {
    const entries = await runPromise(
      withFreshGenerationIds(
        recordGenerationId("gen-real").pipe(
          flatMap(() => recordGenerationId("gen-dummy", true)),
          flatMap(() => recordGenerationId("gen-source", true, true)),
          flatMap(() => getCollectedGenerationIdEntries)
        )
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
      withFreshGenerationIds(
        recordGenerationId("gen-solver", true).pipe(
          flatMap(() =>
            withAuxiliaryUsage(recordGenerationId("gen-judge", true))
          ),
          flatMap(() => getCollectedGenerationIdEntries)
        )
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
      withFreshGenerationIds(
        recordGenerationId("gen-1").pipe(
          flatMap(() => recordGenerationId("")),
          flatMap(() => recordGenerationId(null)),
          flatMap(() => getCollectedGenerationIds)
        )
      )
    );
    expect(ids).toEqual(["gen-1"]);
    expect(
      await runPromise(withFreshGenerationIds(getCollectedGenerationIds))
    ).toEqual([]);
  });
  it("isolates ids recorded by concurrent fibers", async () => {
    const recordThenRead = (id: string) =>
      withFreshGenerationIds(
        recordGenerationId(id).pipe(
          flatMap(() => yieldNow),
          flatMap(() => getCollectedGenerationIds)
        )
      );
    const ids = await runPromise(
      all([recordThenRead("left"), recordThenRead("right")], {
        concurrency: 2,
      })
    );
    expect(ids).toEqual([["left"], ["right"]]);
  });
  it("merges ids recorded by concurrent child fibers back into the parent", async () => {
    const ids = await runPromise(
      withFreshGenerationIds(
        forEach(
          ["gen-a", "gen-b", "gen-c", "gen-d"],
          (id) => recordGenerationId(id),
          {
            concurrency: 4,
          }
        ).pipe(flatMap(() => getCollectedGenerationIds))
      )
    );
    expect([...ids].toSorted()).toEqual(["gen-a", "gen-b", "gen-c", "gen-d"]);
  });

  it("isolates two independent root programs that interleave records", async () => {
    const root = (id: string) =>
      runPromise(
        withFreshGenerationIds(
          recordGenerationId(id).pipe(
            flatMap(() => yieldNow),
            flatMap(() => getCollectedGenerationIds)
          )
        )
      );
    const [first, second] = await Promise.all([root("gen-a"), root("gen-b")]);
    expect(first).toEqual(["gen-a"]);
    expect(second).toEqual(["gen-b"]);
  });

  it("records nothing outside withFreshGenerationIds and leaks nothing into it", async () => {
    const unscoped = await runPromise(
      recordGenerationId("gen-unscoped").pipe(
        flatMap(() => getCollectedGenerationIdEntries)
      )
    );
    expect(unscoped).toEqual([]);
    const scoped = await runPromise(
      withFreshGenerationIds(getCollectedGenerationIds)
    );
    expect(scoped).toEqual([]);
  });
});
