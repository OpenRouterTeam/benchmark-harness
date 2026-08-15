import type { Effect } from "effect/Effect";
import { locally } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

export const RESPONSE_CACHE_HEADER = "x-openrouter-cache";

export const RESPONSE_CACHE_TTL_HEADER = "x-openrouter-cache-ttl";

export const RESPONSE_CACHE_TTL_SECONDS = 7200;

export const RESPONSE_CACHE_STATUS_HEADER = "x-openrouter-cache-status";

export const RESPONSE_CACHE_STATUS_HIT = "HIT";

export const RESPONSE_CACHE_SOURCE_ID_HEADER = "x-openrouter-cache-source-id";

export const RESPONSE_CACHE_SALT_HEADER = "x-openrouter-cache-salt";

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

export const currentCallSaltRef: FiberRef<string | undefined> = unsafeMake<
  string | undefined
>(undefined);

export const getCurrentCallSalt: Effect<string | undefined> =
  get(currentCallSaltRef);

export function withCallCacheSalt<A, E, R>(
  callSalt: string,
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return locally(effect, currentCallSaltRef, callSalt);
}

export function buildResponseCacheSalt(
  sessionId: string | undefined,
  epoch: number | undefined,
  retryAttempt?: number,
  callSalt?: string
): string | undefined {
  const parts = [
    ...(sessionId !== undefined ? [sessionId] : []),
    ...(epoch !== undefined ? [`epoch-${epoch}`] : []),
    ...(callSalt !== undefined ? [callSalt] : []),
    ...(retryAttempt !== undefined && retryAttempt > 0
      ? [`attempt-${retryAttempt}`]
      : []),
  ];
  return parts.length > 0 ? parts.join(":") : undefined;
}
