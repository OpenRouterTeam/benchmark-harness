import { afterEach, describe, expect, it, spyOn } from "bun:test";

import { empty as emptyCause, fail } from "effect/Cause";
import {
  annotateLogs,
  logDebug,
  logError,
  logInfo,
  logTrace,
  logWarning,
  provide,
  runPromise,
  withLogSpan,
} from "effect/Effect";
import { none as noneFiberId } from "effect/FiberId";
import { empty as emptyFiberRefs } from "effect/FiberRefs";
import { empty as emptyHashMap } from "effect/HashMap";
import { empty as emptyList } from "effect/List";
import {
  All,
  Debug,
  Error as ErrorLevel,
  Fatal,
  Info,
  None,
  Trace,
  Warning,
} from "effect/LogLevel";

import {
  harnessEffectLogger,
  harnessLoggerLayer,
  makeHarnessLoggerLayer,
  runHarnessSync,
} from "./effect-logger";
import { emitLog, setLogContextProvider } from "./log";

const originalLogLevel = process.env.LOG_LEVEL;

const originalEnvironment = process.env.OR_ENV;

const originalKService = process.env.K_SERVICE;

const consoleSpies: {
  mockRestore: () => void;
}[] = [];
afterEach(() => {
  for (const consoleSpy of consoleSpies.splice(0)) {
    consoleSpy.mockRestore();
  }
  if (originalLogLevel === undefined) {
    delete process.env.LOG_LEVEL;
  } else {
    process.env.LOG_LEVEL = originalLogLevel;
  }
  if (originalEnvironment === undefined) {
    delete process.env.OR_ENV;
  } else {
    process.env.OR_ENV = originalEnvironment;
  }
  if (originalKService === undefined) {
    delete process.env.K_SERVICE;
  } else {
    process.env.K_SERVICE = originalKService;
  }
  setLogContextProvider(undefined);
});
describe("harness Effect logger", () => {
  it("emits annotations, ambient context, spans, and causes with explicit precedence", async () => {
    process.env.OR_ENV = "production";
    process.env.K_SERVICE = "bench-worker-native";
    process.env.LOG_LEVEL = "4";
    const error = spyOn(console, "error").mockImplementation(() => {});
    setLogContextProvider(() => ({
      workflow_id: "ambient-workflow",
      runtime_id: "ambient-runtime",
    }));
    consoleSpies.push(error);
    await runPromise(
      logError("failed", {
        workflow_id: "explicit-workflow",
      }).pipe(
        annotateLogs({
          workflow_id: "annotated-workflow",
          attempt: 2,
          explicit_field: "annotation",
        }),
        withLogSpan("query"),
        provide(harnessLoggerLayer)
      )
    );
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: fail(new Error("boom")),
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: ErrorLevel,
      message: "failed",
      spans: emptyList(),
    });
    const loggedLine = error.mock.calls[0]?.[0];
    expect(typeof loggedLine).toBe("string");
    expect(loggedLine).toContain('"severity":"ERROR"');
    expect(loggedLine).toContain('"workflow_id":"explicit-workflow"');
    expect(loggedLine).toContain('"runtime_id":"ambient-runtime"');
    expect(loggedLine).toContain('"attempt":2');
    expect(loggedLine).toMatch(/"spans":\{"query":\d+\}/);
    expect(error.mock.calls.some(([line]) => line.includes("boom"))).toBe(true);
  });
  it("maps every Effect log level to a GCP severity without throwing", async () => {
    process.env.OR_ENV = "production";
    process.env.K_SERVICE = "bench-worker-native";
    process.env.LOG_LEVEL = "4";
    const info = spyOn(console, "info").mockImplementation(() => {});
    const warning = spyOn(console, "warn").mockImplementation(() => {});
    const error = spyOn(console, "error").mockImplementation(() => {});
    consoleSpies.push(info, warning, error);
    for (const [logLevel, message] of [
      [All, "all"],
      [Fatal, "fatal"],
      [ErrorLevel, "error"],
      [Warning, "warning"],
      [Info, "info"],
      [Debug, "debug"],
      [Trace, "trace"],
      [None, "none"],
    ] as const) {
      harnessEffectLogger.log({
        annotations: emptyHashMap(),
        cause: emptyCause,
        context: emptyFiberRefs(),
        date: new Date(),
        fiberId: noneFiberId,
        logLevel,
        message,
        spans: emptyList(),
      });
    }
    expect(info.mock.calls.map(([line]) => line)).toEqual([
      expect.stringContaining('"severity":"DEFAULT"'),
      expect.stringContaining('"severity":"INFO"'),
      expect.stringContaining('"severity":"DEBUG"'),
      expect.stringContaining('"severity":"DEBUG"'),
      expect.stringContaining('"severity":"DEFAULT"'),
    ]);
    expect(warning.mock.calls[0]?.[0]).toContain('"severity":"WARNING"');
    expect(error.mock.calls.map(([line]) => line)).toEqual([
      expect.stringContaining('"severity":"CRITICAL"'),
      expect.stringContaining('"severity":"ERROR"'),
    ]);
  });
  it("filters Effect logs according to LOG_LEVEL", async () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    const warning = spyOn(console, "warn").mockImplementation(() => {});
    const error = spyOn(console, "error").mockImplementation(() => {});
    consoleSpies.push(info, warning, error);
    await runPromise(
      logInfo("filter-hidden-info").pipe(provide(makeHarnessLoggerLayer("0")))
    );
    await runPromise(
      logWarning("filter-hidden-warning").pipe(
        provide(makeHarnessLoggerLayer("0"))
      )
    );
    await runPromise(
      logError("filter-visible-error").pipe(
        provide(makeHarnessLoggerLayer("0"))
      )
    );
    expect(
      info.mock.calls.some(([line]) => line.includes("filter-hidden-info"))
    ).toBe(false);
    expect(
      warning.mock.calls.some(([line]) =>
        line.includes("filter-hidden-warning")
      )
    ).toBe(false);
    expect(
      error.mock.calls.some(([line]) => line.includes("filter-visible-error"))
    ).toBe(true);
  });
  it("keeps multi-argument messages as strings and preserves explicit fields", () => {
    process.env.OR_ENV = "production";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: ["hello", 42, { field: "value" }],
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toContain('"message":"hello 42"');
    expect(info.mock.calls[0]?.[0]).toContain('"field":"value"');
  });
  it("preserves fields from a lone plain-object message", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: { field: "value" },
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toBe("");
    expect(info.mock.calls[0]?.[1]).toMatchObject({ field: "value" });
  });
  it("extracts fields when a plain object is the first message argument", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: [{ sample_id: "sample-1" }, "done"],
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toBe("done");
    expect(info.mock.calls[0]?.[1]).toMatchObject({ sample_id: "sample-1" });
  });
  it("extracts plain objects on both sides of message arguments", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: [{ before: true }, "done", { after: true }],
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toBe("done");
    expect(info.mock.calls[0]?.[1]).toMatchObject({
      before: true,
      after: true,
    });
  });
  it("stringifies a lone non-object message value", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: 42,
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toBe("42");
  });
  it("keeps Date and class-instance messages as message text", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    class MessageValue {}
    const date = new Date("2024-01-02T03:04:05.000Z");
    const instance = new MessageValue();
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: [date, instance],
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[0]).toBe(`${date} [object Object]`);
  });
  it("coerces an unserializable message without throwing from the logger", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    class UnserializableMessage {
      toString(): string {
        throw new Error("message conversion failed");
      }
    }
    expect(() =>
      harnessEffectLogger.log({
        annotations: emptyHashMap(),
        cause: emptyCause,
        context: emptyFiberRefs(),
        date: new Date(),
        fiberId: noneFiberId,
        logLevel: Info,
        message: new UnserializableMessage(),
        spans: emptyList(),
      })
    ).not.toThrow();
    expect(info.mock.calls[0]?.[0]).toBe("[Unserializable]");
  });
  it("renders circular and BigInt fields without throwing in production", () => {
    process.env.OR_ENV = "production";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    expect(() =>
      emitLog({
        level: "info",
        message: "unsafe fields",
        context: { circular, count: 2n },
      })
    ).not.toThrow();
    const loggedLine = info.mock.calls.find(([line]) =>
      line.includes('"message":"unsafe fields"')
    )?.[0];
    expect(loggedLine).toContain('"self":"[Circular]"');
    expect(loggedLine).toContain('"count":"2n"');
  });
  it("omits undefined fields from production output", () => {
    process.env.OR_ENV = "production";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    emitLog({
      level: "info",
      message: "optional field",
      context: { present: "yes", absent: undefined },
    });
    const loggedLine = info.mock.calls.find(([line]) =>
      line.includes('"message":"optional field"')
    )?.[0];
    expect(loggedLine).toContain('"present":"yes"');
    expect(loggedLine).not.toContain("absent");
  });
  it("degrades an unserializable payload without dropping severity or ambient context", () => {
    process.env.OR_ENV = "production";
    process.env.K_SERVICE = "bench-worker-native";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    setLogContextProvider(() => ({
      workflow_id: "workflow-serialization",
      runtime_id: "runtime-serialization",
    }));
    const payload = new Proxy(
      {},
      {
        ownKeys: () => {
          throw new Error("payload keys unavailable");
        },
      }
    );
    emitLog({
      level: "info",
      message: "unserializable payload",
      context: { payload },
    });
    const loggedLine = info.mock.calls.find(([line]) =>
      line.includes('"message":"unserializable payload"')
    )?.[0];
    expect(loggedLine).toContain('"severity":"INFO"');
    expect(loggedLine).toContain('"workflow_id":"workflow-serialization"');
    expect(loggedLine).toContain("payload keys unavailable");
  });
  it("serializes shared sibling objects fully more than once", () => {
    process.env.OR_ENV = "production";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    const shared = { value: "shared" };
    emitLog({
      level: "info",
      message: "shared fields",
      context: { first: shared, second: shared },
    });
    const loggedLine = info.mock.calls.find(([line]) =>
      line.includes('"message":"shared fields"')
    )?.[0];
    expect(loggedLine).toContain('"first":{"value":"shared"}');
    expect(loggedLine).toContain('"second":{"value":"shared"}');
    expect(loggedLine).not.toContain("[Circular]");
  });
  it("preserves a __proto__ field as data", () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    const fields: Record<string, unknown> = Object.create(null);
    Object.defineProperty(fields, "__proto__", {
      enumerable: true,
      value: "field-value",
    });
    harnessEffectLogger.log({
      annotations: emptyHashMap(),
      cause: emptyCause,
      context: emptyFiberRefs(),
      date: new Date(),
      fiberId: noneFiberId,
      logLevel: Info,
      message: [fields, "prototype field"],
      spans: emptyList(),
    });
    expect(info.mock.calls[0]?.[1]).toHaveProperty("__proto__", "field-value");
    expect(Object.getPrototypeOf(info.mock.calls[0]?.[1])).toBe(
      Object.prototype
    );
  });
  it("maps each raw LOG_LEVEL index to the expected minimum level", async () => {
    const info = spyOn(console, "info").mockImplementation(() => {});
    const errorSpy = spyOn(console, "error").mockImplementation(() => {});
    consoleSpies.push(info, errorSpy);
    await runPromise(
      logInfo("level-unset").pipe(provide(makeHarnessLoggerLayer(undefined)))
    );
    await runPromise(
      logError("level-negative-hidden").pipe(
        provide(makeHarnessLoggerLayer("-1"))
      )
    );
    await runPromise(
      logInfo("level-zero-hidden").pipe(provide(makeHarnessLoggerLayer("0")))
    );
    await runPromise(
      logInfo("level-one-info").pipe(provide(makeHarnessLoggerLayer("1")))
    );
    await runPromise(
      logInfo("level-two-info").pipe(provide(makeHarnessLoggerLayer("2")))
    );
    await runPromise(
      logDebug("level-three-debug").pipe(provide(makeHarnessLoggerLayer("3")))
    );
    await runPromise(
      logTrace("level-four-trace").pipe(provide(makeHarnessLoggerLayer("4")))
    );
    const infoMessages = info.mock.calls.map(([line]) => line);
    expect(errorSpy).not.toHaveBeenCalled();
    expect(infoMessages.some((line) => line.includes("level-unset"))).toBe(
      true
    );
    expect(
      infoMessages.some((line) => line.includes("level-zero-hidden"))
    ).toBe(false);
    expect(infoMessages.some((line) => line.includes("level-one-info"))).toBe(
      true
    );
    expect(infoMessages.some((line) => line.includes("level-two-info"))).toBe(
      true
    );
    expect(
      infoMessages.some((line) => line.includes("level-three-debug"))
    ).toBe(true);
    expect(infoMessages.some((line) => line.includes("level-four-trace"))).toBe(
      true
    );
  });
  it("runs synchronous effects through the harness runtime", () => {
    process.env.OR_ENV = "production";
    const info = spyOn(console, "info").mockImplementation(() => {});
    consoleSpies.push(info);
    runHarnessSync(logInfo("sync smoke"));
    expect(info.mock.calls[0]?.[0]).toContain('"message":"sync smoke"');
  });
});
