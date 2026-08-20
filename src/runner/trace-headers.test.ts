import { describe, expect, it } from "bun:test";

import { HttpClientRequest } from "@effect/platform";

import { applyTraceHeaders } from "./trace-headers";

const API_PREFIX = "https://openrouter.ai/api/v1";

const TRACE_HEADERS = {
  traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
  "x-benchmark-trace": "test-key",
} as const;

describe("applyTraceHeaders", () => {
  it("adds the headers to a request for the OpenRouter API", () => {
    const request = HttpClientRequest.post(`${API_PREFIX}/chat/completions`);
    const result = applyTraceHeaders(request, API_PREFIX, TRACE_HEADERS);
    expect(result.headers.traceparent).toBe(TRACE_HEADERS.traceparent);
    expect(result.headers["x-benchmark-trace"]).toBe("test-key");
  });

  it("leaves requests to other hosts untouched", () => {
    const request = HttpClientRequest.get(
      "https://datasets-server.huggingface.co/rows"
    );
    const result = applyTraceHeaders(request, API_PREFIX, TRACE_HEADERS);
    expect(result.headers.traceparent).toBeUndefined();
    expect(result.headers["x-benchmark-trace"]).toBeUndefined();
  });
});
