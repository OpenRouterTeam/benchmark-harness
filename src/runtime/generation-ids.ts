import type { Effect } from "effect/Effect";
import { all, flatMap, locally, map, succeed, zipRight } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import {
  get,
  set,
  unsafeMake,
  unsafeMakeHashSet,
  update,
} from "effect/FiberRef";
import { add, empty, has } from "effect/HashSet";

export const generationIdCollector = unsafeMakeHashSet<string>(empty());

export const cacheHitGenerationIdCollector = unsafeMakeHashSet<string>(empty());

export const auxiliaryUsageGenerationIdCollector =
  unsafeMakeHashSet<string>(empty());

export const auxiliaryUsageRef: FiberRef<boolean> = unsafeMake(false);

export function withAuxiliaryUsage<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return locally(effect, auxiliaryUsageRef, true);
}

export function recordGenerationId(
  id: string | null | undefined,
  isCacheHit = false
): Effect<void> {
  if (id === null || id === undefined || id.length === 0) {
    return succeed(undefined);
  }
  const generationId = id;
  let record = update(generationIdCollector, add(generationId));
  if (isCacheHit) {
    record = record.pipe(
      zipRight(update(cacheHitGenerationIdCollector, add(generationId)))
    );
  }
  return get(auxiliaryUsageRef).pipe(
    flatMap((isAuxiliary) =>
      isAuxiliary
        ? record.pipe(
            zipRight(
              update(auxiliaryUsageGenerationIdCollector, add(generationId))
            )
          )
        : record
    )
  );
}

export const resetGenerationIds: Effect<void> = set(
  generationIdCollector,
  empty<string>()
).pipe(
  zipRight(set(cacheHitGenerationIdCollector, empty<string>())),
  zipRight(set(auxiliaryUsageGenerationIdCollector, empty<string>()))
);

export const getCollectedGenerationIds: Effect<readonly string[]> = get(
  generationIdCollector
).pipe(map((ids) => [...ids]));

export interface GenerationIdEntry {
  readonly id: string;
  readonly isCacheHit: boolean;
  readonly countsTowardUsage: boolean;
}

export const getCollectedGenerationIdEntries: Effect<
  readonly GenerationIdEntry[]
> = all([
  get(generationIdCollector),
  get(cacheHitGenerationIdCollector),
  get(auxiliaryUsageGenerationIdCollector),
]).pipe(
  map(([ids, cacheHitIds, auxiliaryIds]) =>
    [...ids].map((id) => ({
      id,
      isCacheHit: has(cacheHitIds, id),
      countsTowardUsage: !has(auxiliaryIds, id),
    }))
  )
);
