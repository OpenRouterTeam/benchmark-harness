import { describe, expect, it } from "bun:test";

import { tryPromiseResult } from "./result";
import { assertFailure, assertSuccess } from "./testing";

describe("tryPromiseResult", () => {
  it("wraps a resolved value in Success", async () => {
    const result = await tryPromiseResult(() => Promise.resolve(42));
    assertSuccess(result);
    expect(result.success).toBe(42);
  });

  it("wraps a rejection in Failure without throwing", async () => {
    const error = new Error("boom");
    const result = await tryPromiseResult(() => Promise.reject(error));
    assertFailure(result);
    expect(result.failure).toBe(error);
  });

  it("wraps a synchronous throw inside the thunk in Failure", async () => {
    const result = await tryPromiseResult(() => {
      throw new TypeError("sync");
    });
    assertFailure(result);
    expect(result.failure).toBeInstanceOf(TypeError);
  });
});
