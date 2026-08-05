import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { AsyncLocalStorage } from "node:async_hooks";

import { eLog, emitLog, iLog, setLogContextProvider, wLog } from "./log";

const originalOrEnv = process.env.OR_ENV;

const originalVercelEnv = process.env.NEXT_PUBLIC_VERCEL_ENV;

const originalKService = process.env.K_SERVICE;

const originalLogLevel = process.env.LOG_LEVEL;

const warnSpies: {
  mockRestore: () => void;
}[] = [];
afterEach(() => {
  for (const warn of warnSpies.splice(0)) {
    warn.mockRestore();
  }
  if (originalOrEnv === undefined) {
    delete process.env.OR_ENV;
  } else {
    process.env.OR_ENV = originalOrEnv;
  }
  if (originalVercelEnv === undefined) {
    delete process.env.NEXT_PUBLIC_VERCEL_ENV;
  } else {
    process.env.NEXT_PUBLIC_VERCEL_ENV = originalVercelEnv;
  }
  if (originalKService === undefined) {
    delete process.env.K_SERVICE;
  } else {
    process.env.K_SERVICE = originalKService;
  }
  if (originalLogLevel === undefined) {
    delete process.env.LOG_LEVEL;
  } else {
    process.env.LOG_LEVEL = originalLogLevel;
  }
  setLogContextProvider(undefined);
});
describe("structured logging shim", () => {
  it("writes production logs as one JSON line with GCP severity", () => {
    process.env.OR_ENV = "production";
    process.env.K_SERVICE = "bench-worker-native";
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    wLog("Retrying after transient error", {
      attempt: 1,
      error_status: 503,
    });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]).toEqual([
      '{"message":"Retrying after transient error","extra":{"attempt":1,"error_status":503},"level":"warn","severity":"WARNING"}',
    ]);
  });
  it("writes production logs without severity outside Cloud Run", () => {
    process.env.OR_ENV = "production";
    delete process.env.K_SERVICE;
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    wLog("Retrying after transient error", { attempt: 1 });
    expect(warn.mock.calls[0]).toEqual([
      '{"message":"Retrying after transient error","extra":{"attempt":1},"level":"warn"}',
    ]);
  });
  it("keeps readable two-argument output outside production", () => {
    process.env.OR_ENV = "development";
    process.env.NEXT_PUBLIC_VERCEL_ENV = "preview";
    delete process.env.K_SERVICE;
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const context = { attempt: 1 };
    wLog("Retrying after transient error", context);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]).toEqual([
      "Retrying after transient error",
      context,
    ]);
  });
  it("propagates ambient context through nested async logs and lets call-site fields win", async () => {
    process.env.OR_ENV = "production";
    delete process.env.K_SERVICE;
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const store = new AsyncLocalStorage<{
      workflow_id: string;
      runtime_id: string;
    }>();
    setLogContextProvider(() => store.getStore());
    await store.run(
      {
        workflow_id: "workflow-123",
        runtime_id: "runtime-123",
      },
      async () => {
        await Promise.resolve();
        wLog("Retrying after transient error", {
          workflow_id: "explicit-workflow",
          attempt: 1,
        });
      }
    );
    expect(warn.mock.calls[0]).toEqual([
      '{"message":"Retrying after transient error","extra":{"workflow_id":"explicit-workflow","runtime_id":"runtime-123","attempt":1},"level":"warn"}',
    ]);
  });
  it("keeps no-context output unchanged", () => {
    process.env.OR_ENV = "development";
    delete process.env.K_SERVICE;
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    wLog("Retrying after transient error");
    expect(warn.mock.calls[0]).toEqual(["Retrying after transient error", {}]);
  });
  it("filters shim logs below the LOG_LEVEL floor like the Effect path", () => {
    process.env.OR_ENV = "development";
    process.env.LOG_LEVEL = "0";
    const info = spyOn(console, "info").mockImplementation(() => {});
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    const error = spyOn(console, "error").mockImplementation(() => {});
    warnSpies.push(info, warn, error);
    iLog("suppressed info");
    wLog("suppressed warning");
    eLog("kept error");
    expect(info).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
    expect(error.mock.calls[0]).toEqual(["kept error", {}]);
  });
  it("keeps warnings at the default LOG_LEVEL floor", () => {
    process.env.OR_ENV = "development";
    delete process.env.LOG_LEVEL;
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    wLog("kept warning");
    expect(warn.mock.calls[0]).toEqual(["kept warning", {}]);
  });
  it("silences every shim level for a negative LOG_LEVEL sentinel", () => {
    process.env.OR_ENV = "development";
    process.env.LOG_LEVEL = "-1";
    const info = spyOn(console, "info").mockImplementation(() => {});
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    const error = spyOn(console, "error").mockImplementation(() => {});
    warnSpies.push(info, warn, error);
    iLog("suppressed info");
    wLog("suppressed warning");
    eLog("suppressed error");
    expect(info).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
    expect(error).not.toHaveBeenCalled();
  });
  it("serializes values produced by successful getters", () => {
    process.env.OR_ENV = "production";
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const payload: Record<string, unknown> = {};
    Object.defineProperty(payload, "field", {
      configurable: true,
      enumerable: true,
      get: () => "computed value",
    });
    emitLog({
      level: "warn",
      message: "accessor field",
      context: { payload },
    });
    expect(warn.mock.calls[0]?.[0]).toContain(
      '"payload":{"field":"computed value"}'
    );
  });
  it("describes errors while sanitizing a throwing field accessor", () => {
    process.env.OR_ENV = "production";
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    let descriptorCalls = 0;
    const payload = new Proxy(
      {},
      {
        ownKeys: () => ["field"],
        getOwnPropertyDescriptor: () => {
          descriptorCalls += 1;
          if (descriptorCalls > 1) {
            throw new Error("field getter failed");
          }
          return {
            configurable: true,
            enumerable: true,
            value: "ignored",
          };
        },
      }
    );
    emitLog({
      level: "warn",
      message: "throwing field accessor",
      context: { payload },
    });
    expect(warn.mock.calls[0]?.[0]).toContain(
      '"payload":{"field":"[Unserializable: field getter failed]"}'
    );
  });
});
