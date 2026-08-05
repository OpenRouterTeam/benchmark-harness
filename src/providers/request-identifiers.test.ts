import { describe, expect, it } from "bun:test";

import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromFetchHeaders,
  modelErrorIdentifiersFromHeaders,
  pickModelErrorIdentifiers,
} from "./request-identifiers";
describe("pickModelErrorIdentifiers", () => {
  it("returns only identifier fields from an error-like object", () => {
    const source = {
      _tag: "ResponsesError",
      retryable: true,
      status: undefined,
      cfRay: "ray-123",
      xRequestId: "req-456",
      generationId: "gen-789",
    };
    expect(pickModelErrorIdentifiers(source)).toEqual({
      cfRay: "ray-123",
      xRequestId: "req-456",
      generationId: "gen-789",
    });
  });
});
describe("appendModelErrorIdentifiers", () => {
  it("leaves messages unchanged when no identifiers are available", () => {
    expect(appendModelErrorIdentifiers("failure", {})).toBe("failure");
  });
  it("appends available identifiers in the canonical order", () => {
    expect(
      appendModelErrorIdentifiers("failure", {
        cfRay: "ray-123",
        generationId: "gen-789",
      })
    ).toBe("failure (cf_ray=ray-123, generation_id=gen-789)");
  });
  it("appends all identifiers when all are available", () => {
    expect(
      appendModelErrorIdentifiers("failure", {
        cfRay: "ray-123",
        xRequestId: "req-456",
        generationId: "gen-789",
      })
    ).toBe(
      "failure (cf_ray=ray-123, x_request_id=req-456, generation_id=gen-789)"
    );
  });
});
describe("modelErrorIdentifiersFromHeaders", () => {
  it("matches header names case-insensitively and omits empty values", () => {
    expect(
      modelErrorIdentifiersFromHeaders({
        "CF-Ray": "ray-123",
        "X-REQUEST-ID": "",
      })
    ).toEqual({ cfRay: "ray-123" });
  });
});
describe("modelErrorIdentifiersFromFetchHeaders", () => {
  it("reads identifiers from Headers and omits missing values", () => {
    const headers = new Headers({ "cf-ray": "ray-123" });
    expect(modelErrorIdentifiersFromFetchHeaders(headers)).toEqual({
      cfRay: "ray-123",
    });
  });
  it("omits identifiers when Headers contains empty values", () => {
    const headers = new Headers({ "cf-ray": "", "x-request-id": "" });
    expect(modelErrorIdentifiersFromFetchHeaders(headers)).toEqual({});
  });
});
