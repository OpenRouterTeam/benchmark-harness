import { Reference } from "effect/Context";
import type { Effect } from "effect/Effect";
import { provideService } from "effect/Effect";

import { definedValues } from "../internal/guards";
import { wLog } from "../internal/log";

export const RESPONSE_CACHE_HEADER = "x-openrouter-cache";

export const RESPONSE_CACHE_TTL_HEADER = "x-openrouter-cache-ttl";

export const RESPONSE_CACHE_TTL_SECONDS = 7200;

export const RESPONSE_CACHE_STATUS_HEADER = "x-openrouter-cache-status";

export const RESPONSE_CACHE_STATUS_HIT = "HIT";

export const RESPONSE_CACHE_SOURCE_ID_HEADER = "x-openrouter-cache-source-id";

export const RESPONSE_CACHE_SALT_HEADER = "x-openrouter-cache-salt";

export const currentEpochRef: Reference<number | undefined> = Reference<
  number | undefined
>("@openrouter/bench-harness/runtime/currentEpoch", {
  defaultValue: () => undefined,
});

export function withCurrentEpoch(
  epoch: number | undefined
): <A, E, R>(effect: Effect<A, E, R>) => Effect<A, E, R> {
  return (effect) => provideService(effect, currentEpochRef, epoch);
}

export const getCurrentEpoch: Effect<number | undefined> = currentEpochRef;

export const currentRetryAttemptRef: Reference<number | undefined> = Reference<
  number | undefined
>("@openrouter/bench-harness/runtime/currentRetryAttempt", {
  defaultValue: () => undefined,
});

export const getCurrentRetryAttempt: Effect<number | undefined> =
  currentRetryAttemptRef;

export const currentRunAttemptRef: Reference<number | undefined> = Reference<
  number | undefined
>("@openrouter/bench-harness/runtime/currentRunAttempt", {
  defaultValue: () => undefined,
});

export const getCurrentRunAttempt: Effect<number | undefined> =
  currentRunAttemptRef;

export function withRunAttempt<A, E, R>(
  runAttempt: number,
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return provideService(effect, currentRunAttemptRef, runAttempt);
}

export const currentCallSaltRef: Reference<string | undefined> = Reference<
  string | undefined
>("@openrouter/bench-harness/runtime/currentCallSalt", {
  defaultValue: () => undefined,
});

export const getCurrentCallSalt: Effect<string | undefined> =
  currentCallSaltRef;

export function withCallCacheSalt<A, E, R>(
  callSalt: string,
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return provideService(effect, currentCallSaltRef, callSalt);
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
