import type { Effect } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

export const RESPONSE_CACHE_HEADER = "x-openrouter-cache";

export const RESPONSE_CACHE_SALT_FIELD = "cache_salt";

export const currentEpochRef: FiberRef<number | undefined> = unsafeMake<
  number | undefined
>(undefined);

export function setCurrentEpoch(epoch: number | undefined): Effect<void> {
  return set(currentEpochRef, epoch);
}

export const getCurrentEpoch: Effect<number | undefined> = get(currentEpochRef);

export function buildResponseCacheSalt(
  sessionId: string | undefined,
  epoch: number | undefined
): string | undefined {
  const parts = [
    ...(sessionId !== undefined ? [sessionId] : []),
    ...(epoch !== undefined ? [`epoch-${epoch}`] : []),
  ];
  return parts.length > 0 ? parts.join(":") : undefined;
}
