import type { DurationInput } from "effect/Duration";
import type { Effect } from "effect/Effect";
import { locally, logWarning, retry, succeed, suspend } from "effect/Effect";
import type { Schedule } from "effect/Schedule";
import {
  exponential,
  jittered,
  modifyDelay,
  onDecision,
  passthrough,
  recurs,
  whileInput,
  zipWith,
} from "effect/Schedule";
import type { ScheduleDecision } from "effect/ScheduleDecision";
import { isContinue } from "effect/ScheduleDecision";
import { start } from "effect/ScheduleIntervals";

import type { ModelError } from "../harness/core";
import {
  isRetryableModelError,
  ModelError as ModelErrorClass,
  SolverError,
} from "../harness/core";
import { unknownErrorToString } from "../internal/errors";
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
  decision: ScheduleDecision
): Effect<void> {
  if (!isContinue(decision)) {
    return succeed(undefined);
  }
  const delayMs = Math.max(0, start(decision.intervals) - Date.now());
  const error = output.error;
  const modelError = error instanceof ModelErrorClass ? error : undefined;
  const errorTag = errorTagFrom(error);
  return logWarning("Retrying after transient error", {
    attempt: output.attempt,
    delay_ms: delayMs,
    error_status: modelError?.status,
    error_tag: errorTag,
    error_message: truncatedMessage(error),
    ...(modelError?.retryAfterMs !== undefined && {
      retry_after_ms: modelError.retryAfterMs,
    }),
    ...(modelError?.cfRay !== undefined && { cf_ray: modelError.cfRay }),
    ...(modelError?.xRequestId !== undefined && {
      x_request_id: modelError.xRequestId,
    }),
    ...(modelError?.generationId !== undefined && {
      generation_id: modelError.generationId,
    }),
  });
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
    zipWith(recurs(maxRetries), (error, attempt) => ({
      error,
      attempt: attempt + 1,
    })),
    onDecision(logRetryDecision)
  );
}

function withRetryAttemptSalt<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  let attempt = -1;
  return suspend(() => {
    attempt += 1;
    return locally(effect, currentRetryAttemptRef, attempt);
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
  const backoff = exponential(`${baseDelayMs} millis`).pipe(jittered);
  const scheduled = backoff.pipe(
    whileInput(isRetryable),
    passthrough,
    modifyDelay((error, computed): DurationInput => {
      const retryAfterMs =
        error instanceof ModelErrorClass ? error.retryAfterMs : undefined;
      return retryAfterMs !== undefined ? `${retryAfterMs} millis` : computed;
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
  const scheduled = whileInput(
    exponential(`${baseDelayMs} millis`).pipe(jittered),
    (error: SolverError): boolean => error instanceof SolverError
  ).pipe(passthrough);
  return withRetryAttemptLogging(scheduled, maxRetries);
}
