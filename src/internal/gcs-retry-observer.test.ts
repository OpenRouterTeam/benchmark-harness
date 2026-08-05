import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";

import type { ApiError } from "@google-cloud/storage";

import { createLoggedRetryableErrorFn } from "./gcs-retry-observer";

const warnSpies: {
  mockRestore: () => void;
}[] = [];
afterEach(() => {
  for (const warn of warnSpies.splice(0)) {
    warn.mockRestore();
  }
});
describe("createLoggedRetryableErrorFn", () => {
  it("delegates retryability and logs retryable errors", () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const predicate = mock<(error: ApiError) => boolean>()
      .mockReturnValueOnce(true)
      .mockReturnValueOnce(false);
    const retryableErrorFn = createLoggedRetryableErrorFn(predicate);
    const error = Object.assign(new Error("unavailable"), { code: 503 });
    expect(retryableErrorFn(error)).toBe(true);
    expect(retryableErrorFn(error)).toBe(false);
    expect(predicate).toHaveBeenCalledTimes(2);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0]?.[0]).toBe("GCS operation hit retryable error");
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      error_message: "unavailable",
      error_code: 503,
    });
  });
  it("caps retryable error messages at 2000 characters", () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const predicate =
      mock<(error: ApiError) => boolean>().mockReturnValue(true);
    const retryableErrorFn = createLoggedRetryableErrorFn(predicate);
    const error = Object.assign(new Error("x".repeat(2500)), { code: 503 });
    expect(retryableErrorFn(error)).toBe(true);
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      error_message: `${"x".repeat(1997)}...`,
    });
  });
});
