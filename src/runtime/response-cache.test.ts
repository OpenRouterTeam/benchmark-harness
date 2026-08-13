import { describe, expect, it } from "bun:test";

import { TaggedError } from "effect/Data";
import { fail, flatMap, retry, runPromise, suspend } from "effect/Effect";
import { recurs } from "effect/Schedule";

import {
  buildResponseCacheSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  setCurrentEpoch,
  withRetryAttemptSalt,
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
});

class BoomError extends TaggedError("BoomError")<{
  readonly message: string;
}> {}

describe("withRetryAttemptSalt", () => {
  it("exposes an attempt index that increments on each retry", async () => {
    const seen: (number | undefined)[] = [];
    const failing = withRetryAttemptSalt(
      getCurrentRetryAttempt.pipe(
        flatMap((attempt) =>
          suspend(() => {
            seen.push(attempt);
            return fail(new BoomError({ message: "boom" }));
          })
        )
      )
    ).pipe(retry(recurs(2)));
    await expect(runPromise(failing)).rejects.toThrow("boom");
    expect(seen).toEqual([0, 1, 2]);
  });
  it("defaults to undefined outside withRetryAttemptSalt", async () => {
    expect(await runPromise(getCurrentRetryAttempt)).toBeUndefined();
  });
  it("restarts at zero for a fresh wrapped effect", async () => {
    const first = await runPromise(
      withRetryAttemptSalt(getCurrentRetryAttempt)
    );
    const second = await runPromise(
      withRetryAttemptSalt(getCurrentRetryAttempt)
    );
    expect(first).toBe(0);
    expect(second).toBe(0);
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
