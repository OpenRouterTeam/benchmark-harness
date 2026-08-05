import type { Effect } from "effect/Effect";
import { map, succeed } from "effect/Effect";
import { get, set, unsafeMakeHashSet, update } from "effect/FiberRef";
import { add, empty } from "effect/HashSet";

export const generationIdCollector = unsafeMakeHashSet<string>(empty());

export function recordGenerationId(
  id: string | null | undefined
): Effect<void> {
  if (id === null || id === undefined || id.length === 0) {
    return succeed(undefined);
  }
  return update(generationIdCollector, add(id));
}

export const resetGenerationIds: Effect<void> = set(
  generationIdCollector,
  empty()
);

export const getCollectedGenerationIds: Effect<readonly string[]> = get(
  generationIdCollector
).pipe(map((ids) => [...ids]));
