import { Reference } from "effect/Context";
import type { Effect } from "effect/Effect";
import {
  flatMap,
  map,
  provideService,
  succeed,
  suspend,
  sync,
} from "effect/Effect";

export interface GenerationIdCollectors {
  readonly ids: Set<string>;
  readonly cacheHitIds: Set<string>;
  readonly auxiliaryUsageIds: Set<string>;
  readonly resolvedSourceIds: Set<string>;
}

function makeGenerationIdCollectors(): GenerationIdCollectors {
  return {
    ids: new Set(),
    cacheHitIds: new Set(),
    auxiliaryUsageIds: new Set(),
    resolvedSourceIds: new Set(),
  };
}

const UNSCOPED_GENERATION_ID_COLLECTORS: GenerationIdCollectors =
  makeGenerationIdCollectors();

export const generationIdCollectors: Reference<GenerationIdCollectors> =
  Reference<GenerationIdCollectors>(
    "@openrouter/bench-harness/runtime/generationIdCollectors",
    { defaultValue: () => UNSCOPED_GENERATION_ID_COLLECTORS }
  );

function isScoped(collectors: GenerationIdCollectors): boolean {
  return collectors !== UNSCOPED_GENERATION_ID_COLLECTORS;
}

export const auxiliaryUsageRef: Reference<boolean> = Reference<boolean>(
  "@openrouter/bench-harness/runtime/auxiliaryUsage",
  { defaultValue: () => false }
);

export function withAuxiliaryUsage<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return provideService(effect, auxiliaryUsageRef, true);
}

export function withFreshGenerationIds<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return suspend(() =>
    provideService(effect, generationIdCollectors, makeGenerationIdCollectors())
  );
}

export function recordGenerationId(
  id: string | null | undefined,
  isCacheHit = false,
  isResolvedSource = false
): Effect<void> {
  if (id === null || id === undefined || id.length === 0) {
    return succeed(undefined);
  }
  const generationId = id;
  return flatMap(generationIdCollectors, (collectors) =>
    flatMap(auxiliaryUsageRef, (isAuxiliary) =>
      sync(() => {
        if (!isScoped(collectors)) {
          return;
        }
        collectors.ids.add(generationId);
        if (isCacheHit) {
          collectors.cacheHitIds.add(generationId);
          if (isResolvedSource) {
            collectors.resolvedSourceIds.add(generationId);
          }
        }
        if (isAuxiliary) {
          collectors.auxiliaryUsageIds.add(generationId);
        }
      })
    )
  );
}

export const getCollectedGenerationIds: Effect<readonly string[]> = map(
  generationIdCollectors,
  (collectors) => [...collectors.ids]
);

export interface GenerationIdEntry {
  readonly id: string;
  readonly isCacheHit: boolean;
  readonly countsTowardUsage: boolean;
  readonly isResolvedSource: boolean;
}

export const getCollectedGenerationIdEntries: Effect<
  readonly GenerationIdEntry[]
> = map(generationIdCollectors, (collectors) =>
  [...collectors.ids].map((id) => ({
    id,
    isCacheHit: collectors.cacheHitIds.has(id),
    countsTowardUsage: !collectors.auxiliaryUsageIds.has(id),
    isResolvedSource: collectors.resolvedSourceIds.has(id),
  }))
);
