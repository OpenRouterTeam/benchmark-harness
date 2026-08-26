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

export const resolvedSourceGenerationIdCollector =
  unsafeMakeHashSet<string>(empty());

export const childGenerationRootCollector = unsafeMakeHashSet<string>(empty());

export const auxiliaryUsageRef: FiberRef<boolean> = unsafeMake(false);

export function withAuxiliaryUsage<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return locally(effect, auxiliaryUsageRef, true);
}

export function recordGenerationId(
  id: string | null | undefined,
  isCacheHit = false,
  isResolvedSource = false,
  shouldResolveChildren = false
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
    if (isResolvedSource) {
      record = record.pipe(
        zipRight(update(resolvedSourceGenerationIdCollector, add(generationId)))
      );
    }
  }
  if (shouldResolveChildren) {
    record = record.pipe(
      zipRight(update(childGenerationRootCollector, add(generationId)))
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
  zipRight(set(auxiliaryUsageGenerationIdCollector, empty<string>())),
  zipRight(set(resolvedSourceGenerationIdCollector, empty<string>())),
  zipRight(set(childGenerationRootCollector, empty<string>()))
);

export const getCollectedGenerationIds: Effect<readonly string[]> = get(
  generationIdCollector
).pipe(map((ids) => [...ids]));

export interface GenerationIdEntry {
  readonly id: string;
  readonly isCacheHit: boolean;
  readonly countsTowardUsage: boolean;
  readonly isResolvedSource: boolean;
  readonly shouldResolveChildren: boolean;
}

export const getCollectedGenerationIdEntries: Effect<
  readonly GenerationIdEntry[]
> = all([
  get(generationIdCollector),
  get(cacheHitGenerationIdCollector),
  get(auxiliaryUsageGenerationIdCollector),
  get(resolvedSourceGenerationIdCollector),
  get(childGenerationRootCollector),
]).pipe(
  map(([ids, cacheHitIds, auxiliaryIds, resolvedSourceIds, childRootIds]) =>
    [...ids].map((id) => ({
      id,
      isCacheHit: has(cacheHitIds, id),
      countsTowardUsage: !has(auxiliaryIds, id),
      isResolvedSource: has(resolvedSourceIds, id),
      shouldResolveChildren: has(childRootIds, id),
    }))
  )
);
