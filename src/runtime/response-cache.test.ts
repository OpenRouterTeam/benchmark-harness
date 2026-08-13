import { describe, expect, it } from "bun:test";

import { flatMap, runPromise } from "effect/Effect";

import {
  buildResponseCacheSalt,
  getCurrentEpoch,
  setCurrentEpoch,
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
