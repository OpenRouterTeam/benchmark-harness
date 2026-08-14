import { describe, expect, it } from "bun:test";

import { all, flatMap, runPromise } from "effect/Effect";

import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  setCurrentEpoch,
  withCallCacheSalt,
} from "./response-cache";

describe("buildResponseCacheSalt", () => {
  it("joins session id and epoch", () => {
    expect(buildResponseCacheSalt("wf-123", 2)).toBe("wf-123:epoch-2");
  });
  it("uses only the session id when epoch is undefined", () => {
    expect(buildResponseCacheSalt("wf-123", undefined)).toBe("wf-123");
  });
  it("uses only the epoch when session id is undefined", () => {
    expect(buildResponseCacheSalt(undefined, 0)).toBe("epoch-0");
  });
  it("returns undefined when both are undefined", () => {
    expect(buildResponseCacheSalt(undefined, undefined)).toBeUndefined();
  });
  it("omits the attempt segment for the first attempt", () => {
    expect(buildResponseCacheSalt("wf-123", 2, 0)).toBe("wf-123:epoch-2");
  });
  it("appends the attempt segment for retries", () => {
    expect(buildResponseCacheSalt("wf-123", 2, 1)).toBe(
      "wf-123:epoch-2:attempt-1"
    );
  });
  it("appends the attempt segment without session or epoch", () => {
    expect(buildResponseCacheSalt(undefined, undefined, 2)).toBe("attempt-2");
  });
  it("places the call salt between the epoch and attempt segments", () => {
    expect(buildResponseCacheSalt("wf-123", 2, 1, "judge-run-3")).toBe(
      "wf-123:epoch-2:judge-run-3:attempt-1"
    );
  });
});

describe("withCallCacheSalt", () => {
  it("scopes the call salt to the wrapped effect", async () => {
    const salts = await runPromise(
      all([
        withCallCacheSalt("judge-run-2", getCurrentCallSalt),
        getCurrentCallSalt,
      ])
    );
    expect(salts).toEqual(["judge-run-2", undefined]);
  });
});

describe("currentRetryAttemptRef", () => {
  it("defaults to undefined", async () => {
    expect(await runPromise(getCurrentRetryAttempt)).toBeUndefined();
  });
});

describe("currentEpochRef", () => {
  it("defaults to undefined", async () => {
    expect(await runPromise(getCurrentEpoch)).toBeUndefined();
  });
  it("returns the epoch set on the current fiber", async () => {
    const epoch = await runPromise(
      setCurrentEpoch(3).pipe(flatMap(() => getCurrentEpoch))
    );
    expect(epoch).toBe(3);
  });
});
