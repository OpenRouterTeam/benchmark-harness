import type { Duration } from "effect/Duration";
import { millis, toMillis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  logWarning,
  provideService,
  retry,
  succeed,
  suspend,
} from "effect/Effect";
import type { Schedule } from "effect/Schedule";
import {
  exponential,
  jittered,
  map,
  modifyDelay,
  passthrough,
  setInputType,
  tap,
  while as while_,
} from "effect/Schedule";

import type { ModelError } from "../harness/core";
import {
  isRetryableModelError,
  ModelError as ModelErrorClass,
  SolverError,
} from "../harness/core";
import { unknownErrorToString } from "../internal/errors";
import { definedValues } from "../internal/guards";
import { currentRetryAttemptRef } from "./response-cache";

type EvalError = ModelError | SolverError;

const MAX_ERROR_MESSAGE_LENGTH = 2000;

export interface RetryConfig {
  readonly maxRetries?: number;
  readonly baseDelayMs?: number;
}

function isRetryable(error: EvalError): boolean {
  if (error instanceof ModelErrorClass) {
    return error.status === undefined || isRetryableModelError(error);
  }
  return false;
}

function logRetryDecision<ErrorType>(
  output: {
    readonly error: ErrorType;
    readonly attempt: number;
  },
  delay: Duration
): Effect<void> {
  const delayMs = Math.max(0, toMillis(delay));
  const error = output.error;
  const modelError = error instanceof ModelErrorClass ? error : undefined;
  const errorTag = errorTagFrom(error);
  return logWarning(
    "Retrying after transient error",
    definedValues({
      attempt: output.attempt,
      delay_ms: delayMs,
      error_status: modelError?.status,
      error_tag: errorTag,
      error_message: truncatedMessage(error),
      retry_after_ms: modelError?.retryAfterMs,
      cf_ray: modelError?.cfRay,
      x_request_id: modelError?.xRequestId,
      generation_id: modelError?.generationId,
    })
  );
}

export function truncatedMessage(error: unknown): string {
  const message = unknownErrorToString(error);
  return message.length > MAX_ERROR_MESSAGE_LENGTH
    ? `${message.slice(0, MAX_ERROR_MESSAGE_LENGTH - 3)}...`
    : message;
}

export function withRetryAttemptLogging<Error>(
  scheduled: Schedule<Error, Error>,
  maxRetries: number
): Schedule<
  {
    readonly error: Error;
    readonly attempt: number;
  },
  Error
> {
  return scheduled.pipe(
    while_(({ attempt }) => attempt <= maxRetries),
    map(({ input, attempt }) => ({ error: input, attempt })),
    tap(({ output, duration }) => logRetryDecision(output, duration))
  );
}

function withRetryAttemptSalt<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  let attempt = -1;
  return suspend(() => {
    attempt += 1;
    return provideService(effect, currentRetryAttemptRef, attempt);
  });
}

export function retrySalted<A, In, E extends In, R>(
  effect: Effect<A, E, R>,
  schedule: Schedule<
    {
      readonly error: In;
      readonly attempt: number;
    },
    In
  >
): Effect<A, E, R> {
  return withRetryAttemptSalt(effect).pipe(retry(schedule));
}

function errorTagFrom(error: unknown): string {
  if (error instanceof ModelErrorClass) {
    return "ModelError";
  }
  if (error instanceof SolverError) {
    return "SolverError";
  }
  if (error instanceof Error) {
    return error.constructor.name;
  }
  return typeof error;
}

export function rateLimitRetrySchedule(config: RetryConfig = {}): Schedule<
  {
    readonly error: EvalError;
    readonly attempt: number;
  },
  EvalError
> {
  const maxRetries = config.maxRetries ?? 6;
  const baseDelayMs = config.baseDelayMs ?? 1e3;
  const backoff = exponential(`${baseDelayMs} millis`).pipe(
    jittered,
    setInputType<EvalError>()
  );
  const scheduled = backoff.pipe(
    while_(({ input }) => isRetryable(input)),
    passthrough,
    modifyDelay(({ input, duration }) => {
      const retryAfterMs =
        input instanceof ModelErrorClass ? input.retryAfterMs : undefined;
      return succeed(
        retryAfterMs !== undefined ? millis(retryAfterMs) : duration
      );
    })
  );
  return withRetryAttemptLogging(scheduled, maxRetries);
}

export function transientSolverRetrySchedule(
  config: RetryConfig = {}
): Schedule<
  {
    readonly error: SolverError;
    readonly attempt: number;
  },
  SolverError
> {
  const maxRetries = config.maxRetries ?? 3;
  const baseDelayMs = config.baseDelayMs ?? 500;
  const scheduled = exponential(`${baseDelayMs} millis`).pipe(
    jittered,
    setInputType<SolverError>(),
    passthrough,
    while_(({ input }) => input instanceof SolverError)
  );
  return withRetryAttemptLogging(scheduled, maxRetries);
}
