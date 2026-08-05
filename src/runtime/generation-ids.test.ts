import { describe, expect, it } from "bun:test";

import { all, flatMap, forEach, runPromise } from "effect/Effect";

import {
  getCollectedGenerationIds,
  recordGenerationId,
  resetGenerationIds,
} from "./generation-ids";
describe("generation id collector", () => {
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
