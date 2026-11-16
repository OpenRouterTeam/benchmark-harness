import { describe, expect, it } from "bun:test";

import { requestedByHeaders } from "./app-identity";

describe("requestedByHeaders", () => {
  it("sends the requester from OPENROUTER_REQUESTED_BY", () => {
    expect(requestedByHeaders({ OPENROUTER_REQUESTED_BY: "jane.doe" })).toEqual(
      { "X-OpenRouter-Requested-By": "jane.doe" }
    );
  });

  it("sends nothing when the requester is unset or empty", () => {
    expect(requestedByHeaders({})).toEqual({});
    expect(requestedByHeaders({ OPENROUTER_REQUESTED_BY: "" })).toEqual({});
  });
});
