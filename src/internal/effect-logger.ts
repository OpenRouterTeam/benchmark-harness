import type { Cause } from "effect/Cause";
import { isEmpty as isCauseEmpty, pretty as prettyCause } from "effect/Cause";
import type { Effect } from "effect/Effect";
import { sync } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { mergeAll, unwrapEffect } from "effect/Layer";
import { toArray } from "effect/List";
import { defaultLogger, make, minimumLogLevel, replace } from "effect/Logger";
import type { LogLevel } from "effect/LogLevel";
import { Debug, Error as ErrorLevel, Info, None, Trace } from "effect/LogLevel";
import type { LogSpan } from "effect/LogSpan";
import type { ManagedRuntime } from "effect/ManagedRuntime";
import { make as makeManagedRuntime } from "effect/ManagedRuntime";

import { definedValues } from "./guards";
import {
  emitLog,
  GCP_SEVERITY_BY_LEVEL,
  resolveMinimumHarnessLogLevel,
} from "./log";

const EMIT_LEVEL_BY_LOG_LEVEL = {
  ALL: "all",
  FATAL: "fatal",
  ERROR: "error",
  WARN: "warn",
  INFO: "info",
  DEBUG: "debug",
  TRACE: "trace",
  OFF: "none",
} as const satisfies Record<
  LogLevel["label"],
  keyof typeof GCP_SEVERITY_BY_LEVEL
>;

export const harnessEffectLogger = make<unknown, void>(
  ({ annotations, cause, logLevel, message, spans }) => {
    const normalizedMessage = normalizeMessage(message);
    const renderedSpans = renderSpans(toArray(spans), Date.now());
    const renderedCause = renderCause(cause);
    const context = {
      ...Object.fromEntries(annotations),
      ...normalizedMessage.context,
      ...definedValues({
        spans:
          Object.keys(renderedSpans).length > 0 ? renderedSpans : undefined,
      }),
      ...definedValues({
        cause: renderedCause,
      }),
    };
    emitLog({
      level: EMIT_LEVEL_BY_LOG_LEVEL[logLevel.label],
      message: normalizedMessage.message,
      context,
    });
  }
);

export function makeHarnessLoggerLayer(
  rawLogLevel: string | undefined
): Layer<never> {
  return mergeAll(
    replace(defaultLogger, harnessEffectLogger),
    minimumLogLevel(resolveMinimumLogLevel(rawLogLevel))
  );
}

export const harnessLoggerLayer: Layer<never> = unwrapEffect(
  sync(() => makeHarnessLoggerLayer(process.env.LOG_LEVEL))
);

let harnessRuntime: ManagedRuntime<never, never> | undefined;

export function runHarnessPromise<A, E>(
  effect: Effect<A, E, never>,
  options?: {
    readonly signal?: AbortSignal;
  }
): Promise<A> {
  return getHarnessRuntime().runPromise(effect, options);
}

export function runHarnessSync<A, E>(effect: Effect<A, E, never>): A {
  return getHarnessRuntime().runSync(effect);
}

function getHarnessRuntime(): ManagedRuntime<never, never> {
  if (harnessRuntime === undefined) {
    harnessRuntime = makeManagedRuntime(harnessLoggerLayer);
  }
  return harnessRuntime;
}

function resolveMinimumLogLevel(raw: string | undefined): LogLevel {
  const level = resolveMinimumHarnessLogLevel(raw);
  switch (level) {
    case "none": {
      return None;
    }
    case "error": {
      return ErrorLevel;
    }
    case "info": {
      return Info;
    }
    case "debug": {
      return Debug;
    }
    case "trace": {
      return Trace;
    }
    default: {
      return level satisfies never;
    }
  }
}

function renderSpans(
  spans: readonly LogSpan[],
  now: number
): Record<string, number> {
  return Object.fromEntries(
    spans.map((span) => [span.label, now - span.startTime])
  );
}

function renderCause(cause: Cause<unknown>): string | undefined {
  return isCauseEmpty(cause) ? undefined : prettyCause(cause);
}

function normalizeMessage(message: unknown): {
  readonly message: string;
  readonly context: Record<string, unknown>;
} {
  const messageValues = Array.isArray(message) ? message : [message];
  const explicitFields = messageValues.filter(isLogFields);
  const messageArguments = messageValues.filter((value) => !isLogFields(value));
  return {
    message: messageArguments.map(stringifyMessageValue).join(" "),
    context: Object.fromEntries(
      explicitFields.flatMap((fields) => Object.entries(fields))
    ),
  };
}

function stringifyMessageValue(value: unknown): string {
  try {
    return String(value);
  } catch {
    return "[Unserializable]";
  }
}

function isLogFields(value: unknown): value is Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    (Object.getPrototypeOf(value) === Object.prototype ||
      Object.getPrototypeOf(value) === null)
  );
}
