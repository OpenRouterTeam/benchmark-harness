import type { Mock } from "bun:test";
import { afterEach, describe, expect, it, spyOn } from "bun:test";

import { all, flatMap, runPromise } from "effect/Effect";

import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  getCurrentRunAttempt,
  logUnexpectedResponseCacheMiss,
  setCurrentEpoch,
  shouldExpectResponseCacheHit,
  withCallCacheSalt,
  withRunAttempt,
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

describe("currentRunAttemptRef", () => {
  it("defaults to undefined", async () => {
    expect(await runPromise(getCurrentRunAttempt)).toBeUndefined();
  });
  it("scopes the run attempt to the wrapped effect", async () => {
    const attempts = await runPromise(
      all([withRunAttempt(2, getCurrentRunAttempt), getCurrentRunAttempt])
    );
    expect(attempts).toEqual([2, undefined]);
  });
});

describe("shouldExpectResponseCacheHit", () => {
  it("expects a hit on a retried run replaying the same salt", () => {
    expect(
      shouldExpectResponseCacheHit({
        runAttempt: 2,
        retryAttempt: 0,
        cacheSalt: "wf-123:epoch-0",
      })
    ).toBe(true);
  });
  it("expects nothing on the first run attempt", () => {
    expect(
      shouldExpectResponseCacheHit({
        runAttempt: 1,
        retryAttempt: 0,
        cacheSalt: "wf-123:epoch-0",
      })
    ).toBe(false);
  });
  it("expects nothing when the run attempt is unknown", () => {
    expect(
      shouldExpectResponseCacheHit({
        runAttempt: undefined,
        retryAttempt: 0,
        cacheSalt: "wf-123:epoch-0",
      })
    ).toBe(false);
  });
  it("expects nothing for in-process retries that change the salt", () => {
    expect(
      shouldExpectResponseCacheHit({
        runAttempt: 3,
        retryAttempt: 1,
        cacheSalt: "wf-123:epoch-0:attempt-1",
      })
    ).toBe(false);
  });
  it("expects nothing when no cache salt is sent", () => {
    expect(
      shouldExpectResponseCacheHit({
        runAttempt: 2,
        retryAttempt: 0,
        cacheSalt: undefined,
      })
    ).toBe(false);
  });
});

describe("logUnexpectedResponseCacheMiss", () => {
  let warn: Mock<(...args: unknown[]) => void> | undefined;
  afterEach(() => {
    warn?.mockRestore();
    warn = undefined;
  });
  function silenceWarnings(): Mock<(...args: unknown[]) => void> {
    warn = spyOn(console, "warn").mockImplementation(() => {});
    return warn;
  }
  it("logs the cache salt and attempt when an expected hit misses", () => {
    const spy = silenceWarnings();
    logUnexpectedResponseCacheMiss({
      isCacheHit: false,
      runAttempt: 2,
      retryAttempt: 0,
      cacheSalt: "wf-123:epoch-0",
      model: "openai/gpt-4o",
      cacheStatus: "MISS",
      cfRay: "ray-1",
      generationId: "gen-1",
    });
    expect(spy).toHaveBeenCalledTimes(1);
    const [message, context] = spy.mock.calls[0] ?? [];
    expect(message).toBe("Expected response cache hit on run retry but missed");
    expect(context).toMatchObject({
      run_attempt: 2,
      cache_salt: "wf-123:epoch-0",
      model: "openai/gpt-4o",
      cache_status: "MISS",
      cf_ray: "ray-1",
      generation_id: "gen-1",
    });
  });
  it("stays silent when the retried run hits the cache", () => {
    const spy = silenceWarnings();
    logUnexpectedResponseCacheMiss({
      isCacheHit: true,
      runAttempt: 2,
      retryAttempt: 0,
      cacheSalt: "wf-123:epoch-0",
    });
    expect(spy).not.toHaveBeenCalled();
  });
  it("stays silent on a first-attempt miss", () => {
    const spy = silenceWarnings();
    logUnexpectedResponseCacheMiss({
      isCacheHit: false,
      runAttempt: 1,
      retryAttempt: 0,
      cacheSalt: "wf-123:epoch-0",
    });
    expect(spy).not.toHaveBeenCalled();
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
