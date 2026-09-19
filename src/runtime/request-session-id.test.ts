import { describe, expect, it } from "bun:test";

import {
  buildRequestSessionId,
  REQUEST_SESSION_ID_MAX_LENGTH,
} from "./request-session-id";

describe("buildRequestSessionId", () => {
  it("nests epoch and sample id under the run session id", () => {
    expect(buildRequestSessionId("wf-123", 0, "gpqa-42")).toBe(
      "wf-123.0.gpqa-42"
    );
  });
  it("returns undefined without a run session id", () => {
    expect(buildRequestSessionId(undefined, 0, "s")).toBeUndefined();
  });
  it("falls back to the run session id when epoch or sample is unknown", () => {
    expect(buildRequestSessionId("wf-123", undefined, "s")).toBe("wf-123");
    expect(buildRequestSessionId("wf-123", 1, undefined)).toBe("wf-123");
  });
  it("sanitizes sample id characters outside the safe set and appends a hash of the original", () => {
    expect(buildRequestSessionId("wf", 1, "django__django-1234 b.c/d")).toBe(
      "wf.1.django__django-1234-b-c-d-1769c4cf0346"
    );
  });
  it("keeps safe sample ids verbatim without a hash", () => {
    expect(buildRequestSessionId("wf", 1, "q-17")).toBe("wf.1.q-17");
  });
  it("keeps punctuation variants of a sample id distinct", () => {
    const ids = ["q/17", "q.17", "q 17", "q-17"].map((sampleId) =>
      buildRequestSessionId("wf", 1, sampleId)
    );
    expect(new Set(ids).size).toBe(ids.length);
  });
  it("hashes the tail of overlong sample ids and stays within the bound", () => {
    const sampleId = "x".repeat(300);
    const other = `${"x".repeat(299)}y`;
    const a = buildRequestSessionId("wf-123", 2, sampleId);
    const b = buildRequestSessionId("wf-123", 2, other);
    expect(a?.length).toBeLessThanOrEqual(REQUEST_SESSION_ID_MAX_LENGTH);
    expect(a?.startsWith("wf-123.2.")).toBe(true);
    expect(a).not.toBe(b);
  });
  it("hashes the full identity when the run session id leaves no room for a sample segment", () => {
    const sessionId = "s".repeat(REQUEST_SESSION_ID_MAX_LENGTH - 2);
    const a = buildRequestSessionId(sessionId, 0, "abc");
    const b = buildRequestSessionId(sessionId, 0, "abd");
    const c = buildRequestSessionId(sessionId, 1, "abc");
    expect(a?.length).toBe(REQUEST_SESSION_ID_MAX_LENGTH);
    expect(a).not.toBe(sessionId);
    expect(new Set([a, b, c]).size).toBe(3);
    expect(a).toBe(buildRequestSessionId(sessionId, 0, "abc"));
  });
  it("stays within the bound for a maximum-length run session id", () => {
    const sessionId = "s".repeat(REQUEST_SESSION_ID_MAX_LENGTH);
    const a = buildRequestSessionId(sessionId, 3, "sample/1");
    expect(a?.length).toBeLessThanOrEqual(REQUEST_SESSION_ID_MAX_LENGTH);
    expect(a).not.toBe(sessionId);
  });
});
