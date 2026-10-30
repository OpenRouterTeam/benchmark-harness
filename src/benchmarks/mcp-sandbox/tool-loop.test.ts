import { describe, expect, it } from "bun:test";

import * as Result from "effect/Result";

import { capToolOutput, parseToolArguments } from "./tool-loop";

describe("capToolOutput", () => {
  it("returns short output untouched", () => {
    expect(capToolOutput("abc", 10)).toBe("abc");
    expect(capToolOutput("abc", undefined)).toBe("abc");
  });

  it("truncates and labels long output", () => {
    const capped = capToolOutput("abcdef", 3);
    expect(capped.startsWith("abc\n")).toBe(true);
    expect(capped).toContain("truncated to 3 chars");
  });
});

describe("parseToolArguments", () => {
  it("parses a JSON object", () => {
    const parsed = parseToolArguments('{"a": 1}');
    expect(Result.isSuccess(parsed) && parsed.success).toEqual({ a: 1 });
  });

  it("treats an empty string as no arguments", () => {
    const parsed = parseToolArguments("");
    expect(Result.isSuccess(parsed) && parsed.success).toEqual({});
  });

  it("rejects non-object JSON", () => {
    expect(Result.isFailure(parseToolArguments("[1]"))).toBe(true);
    expect(Result.isFailure(parseToolArguments("{nope"))).toBe(true);
  });
});
