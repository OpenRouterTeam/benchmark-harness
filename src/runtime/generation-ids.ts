import type { Effect } from "effect/Effect";
import { all, map, succeed, zipRight } from "effect/Effect";
import { get, set, unsafeMakeHashSet, update } from "effect/FiberRef";
import { add, empty, has } from "effect/HashSet";

export const generationIdCollector = unsafeMakeHashSet<string>(empty());

export const cacheHitGenerationIdCollector = unsafeMakeHashSet<string>(empty());

export function recordGenerationId(
  id: string | null | undefined,
  isCacheHit = false
): Effect<void> {
  if (id === null || id === undefined || id.length === 0) {
    return succeed(undefined);
  }
  const record = update(generationIdCollector, add(id));
  return isCacheHit
    ? record.pipe(zipRight(update(cacheHitGenerationIdCollector, add(id))))
    : record;
}

export const resetGenerationIds: Effect<void> = set(
  generationIdCollector,
  empty<string>()
).pipe(zipRight(set(cacheHitGenerationIdCollector, empty<string>())));

export const getCollectedGenerationIds: Effect<readonly string[]> = get(
  generationIdCollector
).pipe(map((ids) => [...ids]));

export interface GenerationIdEntry {
  readonly id: string;
  readonly isCacheHit: boolean;
}

export const getCollectedGenerationIdEntries: Effect<
  readonly GenerationIdEntry[]
> = all([get(generationIdCollector), get(cacheHitGenerationIdCollector)]).pipe(
  map(([ids, cacheHitIds]) =>
    [...ids].map((id) => ({ id, isCacheHit: has(cacheHitIds, id) }))
  )
);
