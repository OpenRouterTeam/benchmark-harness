import { describe, expect, it } from "bun:test";

import {
  isRetryableModelError,
  isSystemicModelError,
  ModelError,
} from "./core";

describe("isRetryableModelError", () => {
  it("treats 408 request timeout as retryable", () => {
    const error = new ModelError({ status: 408, message: "Request Timeout" });
    expect(isRetryableModelError(error)).toBe(true);
  });

  it("treats 429 rate limit as retryable", () => {
    const error = new ModelError({ status: 429, message: "Too Many Requests" });
    expect(isRetryableModelError(error)).toBe(true);
  });

  it("treats 5xx server errors as retryable", () => {
    for (const status of [500, 502, 503, 504]) {
      const error = new ModelError({
        status,
        message: `Server error ${status}`,
      });
      expect(isRetryableModelError(error)).toBe(true);
    }
  });

  it("does not treat client errors other than 408 and 429 as retryable", () => {
    for (const status of [400, 401, 403, 404, 422]) {
      const error = new ModelError({
        status,
        message: `Client error ${status}`,
      });
      expect(isRetryableModelError(error)).toBe(false);
    }
  });

  it("does not treat undefined status as retryable", () => {
    const error = new ModelError({ message: "Unknown error" });
    expect(isRetryableModelError(error)).toBe(false);
  });
});

describe("isSystemicModelError", () => {
  it("treats 401, 403, and 404 as systemic", () => {
    for (const status of [401, 403, 404]) {
      const error = new ModelError({ status, message: `Systemic ${status}` });
      expect(isSystemicModelError(error)).toBe(true);
    }
  });

  it("treats undefined status as systemic", () => {
    const error = new ModelError({ message: "Unclassified error" });
    expect(isSystemicModelError(error)).toBe(true);
  });

  it("does not treat 408, 429, or 5xx as systemic", () => {
    for (const status of [408, 429, 500, 502, 503, 504]) {
      const error = new ModelError({ status, message: `Transient ${status}` });
      expect(isSystemicModelError(error)).toBe(false);
    }
  });
});
