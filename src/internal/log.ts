import { unknownErrorToString } from "./errors";
import { definedValues } from "./guards";

export type LogContext = Record<string, unknown>;

type AmbientLogContext = {
  workflow_id: string;
  runtime_id: string;
};

type LogContextProvider = () => AmbientLogContext | undefined;

let logContextProvider: LogContextProvider | undefined;

export function setLogContextProvider(
  provider: LogContextProvider | undefined
): void {
  logContextProvider = provider;
}

export const GCP_SEVERITY_BY_LEVEL = {
  all: "DEFAULT",
  fatal: "CRITICAL",
  error: "ERROR",
  warn: "WARNING",
  info: "INFO",
  debug: "DEBUG",
  trace: "DEBUG",
  none: "DEFAULT",
} as const;

export type LogLevel = keyof typeof GCP_SEVERITY_BY_LEVEL;

export type ConsoleLogLevel = "info" | "warn" | "error";

export type MinimumHarnessLogLevel =
  | "none"
  | "error"
  | "info"
  | "debug"
  | "trace";

export function resolveMinimumHarnessLogLevel(
  raw: string | undefined
): MinimumHarnessLogLevel {
  if (raw === undefined) {
    return "info";
  }
  const value = Number.parseInt(raw, 10);
  if (value < 0) {
    return "none";
  }
  switch (value) {
    case 0: {
      return "error";
    }
    case 1:
    case 2: {
      return "info";
    }
    case 3: {
      return "debug";
    }
    case 4: {
      return "trace";
    }
    default: {
      return "info";
    }
  }
}

const LOG_LEVEL_RANK = {
  all: 0,
  trace: 1,
  debug: 2,
  info: 3,
  warn: 4,
  error: 5,
  fatal: 6,
  none: 7,
} as const satisfies Record<LogLevel, number>;

function shouldEmit(level: LogLevel): boolean {
  const floor = resolveMinimumHarnessLogLevel(process.env.LOG_LEVEL);
  return LOG_LEVEL_RANK[level] >= LOG_LEVEL_RANK[floor];
}

export interface EmitLogOptions {
  readonly level: LogLevel;
  readonly message: string;
  readonly context?: LogContext;
}

function isProduction(): boolean {
  return (
    process.env.NEXT_PUBLIC_VERCEL_ENV === "production" ||
    process.env.OR_ENV === "production"
  );
}

export function emitLog({ level, message, context }: EmitLogOptions): void {
  const ambientContext = getAmbientLogContext();
  const extra = {
    ...ambientContext,
    ...context,
  };
  if (isProduction()) {
    const record = {
      message,
      extra,
      level,
      ...definedValues({
        severity: process.env.K_SERVICE
          ? GCP_SEVERITY_BY_LEVEL[level]
          : undefined,
      }),
    };
    console[CONSOLE_LEVEL_BY_LEVEL[level]](stringifyLogRecord(record));
    return;
  }
  console[CONSOLE_LEVEL_BY_LEVEL[level]](message, extra);
}

const CONSOLE_LEVEL_BY_LEVEL = {
  all: "info",
  fatal: "error",
  error: "error",
  warn: "warn",
  info: "info",
  debug: "info",
  trace: "info",
  none: "info",
} as const satisfies Record<LogLevel, ConsoleLogLevel>;

function getAmbientLogContext(): AmbientLogContext | undefined {
  return logContextProvider?.();
}

function stringifyLogRecord(record: Record<string, unknown>): string {
  const safeRecord = {
    ...record,
    extra: sanitizeLogValue(record.extra, []),
  };
  return JSON.stringify(safeRecord);
}

function sanitizeLogValue(
  value: unknown,
  ancestors: readonly object[]
): unknown {
  if (typeof value === "bigint") {
    return `${value}n`;
  }
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (value === undefined) {
    return undefined;
  }
  if (typeof value === "symbol" || typeof value === "function") {
    return undefined;
  }
  if (ancestors.includes(value)) {
    return "[Circular]";
  }
  if (ancestors.length >= 100) {
    return "[MaxDepth]";
  }
  const nextAncestors = [...ancestors, value];
  if (Array.isArray(value)) {
    return value.map((item) => sanitizeLogValue(item, nextAncestors));
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    return safeString(value);
  }
  let keys: string[];
  try {
    keys = Object.keys(value);
  } catch (error) {
    return `[Unserializable: ${safeErrorString(error)}]`;
  }
  const entries = keys.map((key) => {
    try {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      const rawValue =
        descriptor !== undefined && "value" in descriptor
          ? descriptor.value
          : Reflect.get(value, key);
      return [key, sanitizeLogValue(rawValue, nextAncestors)] as const;
    } catch (error) {
      return [key, `[Unserializable: ${safeErrorString(error)}]`] as const;
    }
  });
  return Object.fromEntries(entries);
}

function safeErrorString(error: unknown): string {
  try {
    return unknownErrorToString(error);
  } catch {
    return "[Unserializable error]";
  }
}

function safeString(value: unknown): string {
  try {
    return String(value);
  } catch {
    return "[Unserializable]";
  }
}

export function iLog(message: string, context?: LogContext): void {
  if (shouldEmit("info")) {
    emitLog({ level: "info", message, context });
  }
}

export function wLog(message: string, context?: LogContext): void {
  if (shouldEmit("warn")) {
    emitLog({ level: "warn", message, context });
  }
}

export function eLog(message: string, context?: LogContext): void {
  if (shouldEmit("error")) {
    emitLog({ level: "error", message, context });
  }
}
