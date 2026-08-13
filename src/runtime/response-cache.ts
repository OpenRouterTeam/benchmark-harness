import type { Effect } from "effect/Effect";
import { locally, suspend } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

export const RESPONSE_CACHE_HEADER = "x-openrouter-cache";

export const RESPONSE_CACHE_SALT_FIELD = "cache_salt";

export const RESPONSE_CACHE_SOURCE_GENERATION_HEADER =
  "x-openrouter-cache-source-generation-id";

export const currentEpochRef: FiberRef<number | undefined> = unsafeMake<
  number | undefined
>(undefined);

export function setCurrentEpoch(epoch: number | undefined): Effect<void> {
  return set(currentEpochRef, epoch);
}

export const getCurrentEpoch: Effect<number | undefined> = get(currentEpochRef);

export const currentRetryAttemptRef: FiberRef<number | undefined> = unsafeMake<
  number | undefined
>(undefined);

export const getCurrentRetryAttempt: Effect<number | undefined> = get(
  currentRetryAttemptRef
);

export function withRetryAttemptSalt<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  let attempt = -1;
  return suspend(() => {
    attempt += 1;
    return locally(effect, currentRetryAttemptRef, attempt);
  });
}

export function buildResponseCacheSalt(
  sessionId: string | undefined,
  epoch: number | undefined,
  retryAttempt?: number
): string | undefined {
  const parts = [
    ...(sessionId !== undefined ? [sessionId] : []),
    ...(epoch !== undefined ? [`epoch-${epoch}`] : []),
    ...(retryAttempt !== undefined && retryAttempt > 0
      ? [`attempt-${retryAttempt}`]
      : []),
  ];
  return parts.length > 0 ? parts.join(":") : undefined;
}
