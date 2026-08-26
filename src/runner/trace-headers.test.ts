import { describe, expect, it } from "bun:test";

import { filterTraceHeaders } from "./trace-headers";

const TRACEPARENT = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

describe("filterTraceHeaders", () => {
  it("keeps the allowed trace header names", () => {
    expect(
      filterTraceHeaders({
        traceparent: TRACEPARENT,
        tracestate: "or=bench",
        "x-or-traceparent": TRACEPARENT,
        "x-benchmark-trace": "test-key",
      })
    ).toEqual({
      traceparent: TRACEPARENT,
      tracestate: "or=bench",
      "x-or-traceparent": TRACEPARENT,
      "x-benchmark-trace": "test-key",
    });
  });

  it("normalizes header names to lowercase", () => {
    expect(filterTraceHeaders({ Traceparent: TRACEPARENT })).toEqual({
      traceparent: TRACEPARENT,
    });
  });

  it("drops disallowed header names such as authorization", () => {
    expect(
      filterTraceHeaders({
        traceparent: TRACEPARENT,
        authorization: "Bearer attacker-key",
        "x-session-id": "spoofed",
      })
    ).toEqual({ traceparent: TRACEPARENT });
  });

  it("returns undefined when nothing survives the filter", () => {
    expect(filterTraceHeaders({ authorization: "Bearer x" })).toBeUndefined();
    expect(filterTraceHeaders(undefined)).toBeUndefined();
  });
});
