import type { Cause } from "effect/Cause";
import { pretty as prettyCause } from "effect/Cause";
import type { Effect } from "effect/Effect";
import { sync } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { mergeAll, succeed as layerSucceed, unwrap } from "effect/Layer";
import { layer as loggerLayer, make } from "effect/Logger";
import type { LogLevel } from "effect/LogLevel";
import type { ManagedRuntime } from "effect/ManagedRuntime";
import { make as makeManagedRuntime } from "effect/ManagedRuntime";
import {
  CurrentLogAnnotations,
  CurrentLogSpans,
  MinimumLogLevel,
} from "effect/References";

import { definedValues } from "./guards";
import {
  emitLog,
  GCP_SEVERITY_BY_LEVEL,
  resolveMinimumHarnessLogLevel,
} from "./log";

const EMIT_LEVEL_BY_LOG_LEVEL = {
  All: "all",
  Fatal: "fatal",
  Error: "error",
  Warn: "warn",
  Info: "info",
  Debug: "debug",
  Trace: "trace",
  None: "none",
} as const satisfies Record<LogLevel, keyof typeof GCP_SEVERITY_BY_LEVEL>;

export const harnessEffectLogger = make<unknown, void>(
  ({ cause, fiber, logLevel, message }) => {
    const annotations = fiber.getRef(CurrentLogAnnotations);
    const spans = fiber.getRef(CurrentLogSpans);
    const normalizedMessage = normalizeMessage(message);
    const renderedSpans = renderSpans(spans, Date.now());
    const renderedCause = renderCause(cause);
    const context = {
      ...annotations,
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
      level: EMIT_LEVEL_BY_LOG_LEVEL[logLevel],
      message: normalizedMessage.message,
      context,
    });
  }
);

export function makeHarnessLoggerLayer(
  rawLogLevel: string | undefined
): Layer<never> {
  return mergeAll(
    loggerLayer([harnessEffectLogger]),
    layerSucceed(MinimumLogLevel, resolveMinimumLogLevel(rawLogLevel))
  );
}

export const harnessLoggerLayer: Layer<never> = unwrap(
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
      return "None";
    }
    case "error": {
      return "Error";
    }
    case "info": {
      return "Info";
    }
    case "debug": {
      return "Debug";
    }
    case "trace": {
      return "Trace";
    }
    default: {
      return level satisfies never;
    }
  }
}

function renderSpans(
  spans: readonly (readonly [label: string, timestamp: number])[],
  now: number
): Record<string, number> {
  return Object.fromEntries(
    spans.map(([label, startTime]) => [label, now - startTime])
  );
}

function renderCause(cause: Cause<unknown>): string | undefined {
  return cause.reasons.length === 0 ? undefined : prettyCause(cause);
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
