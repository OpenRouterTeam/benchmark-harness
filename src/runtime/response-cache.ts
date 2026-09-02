import type { Effect } from "effect/Effect";
import { locally } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

import { definedValues } from "../internal/guards";
import { wLog } from "../internal/log";

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

export const currentRunAttemptRef: FiberRef<number | undefined> = unsafeMake<
  number | undefined
>(undefined);

export const getCurrentRunAttempt: Effect<number | undefined> =
  get(currentRunAttemptRef);

export function withRunAttempt<A, E, R>(
  runAttempt: number,
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return locally(effect, currentRunAttemptRef, runAttempt);
}

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

export interface ResponseCacheAttemptState {
  readonly runAttempt: number | undefined;
  readonly retryAttempt: number | undefined;
  readonly cacheSalt: string | undefined;
}

export function shouldExpectResponseCacheHit({
  runAttempt,
  retryAttempt,
  cacheSalt,
}: ResponseCacheAttemptState): boolean {
  return (
    cacheSalt !== undefined &&
    runAttempt !== undefined &&
    runAttempt > 1 &&
    (retryAttempt === undefined || retryAttempt <= 0)
  );
}

export interface ResponseCacheMissContext extends ResponseCacheAttemptState {
  readonly isCacheHit: boolean;
  readonly model?: string;
  readonly cacheStatus?: string;
  readonly cfRay?: string;
  readonly xRequestId?: string;
  readonly generationId?: string;
}

export function logUnexpectedResponseCacheMiss(
  context: ResponseCacheMissContext
): void {
  if (context.isCacheHit || !shouldExpectResponseCacheHit(context)) {
    return;
  }
  wLog(
    "Expected response cache hit on run retry but missed",
    definedValues({
      run_attempt: context.runAttempt,
      cache_salt: context.cacheSalt,
      model: context.model,
      cache_status: context.cacheStatus,
      cf_ray: context.cfRay,
      x_request_id: context.xRequestId,
      generation_id: context.generationId,
    })
  );
}
