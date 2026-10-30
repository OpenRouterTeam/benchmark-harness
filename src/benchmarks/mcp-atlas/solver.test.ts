import { describe, expect, it } from "bun:test";

import * as Result from "effect/Result";

import { capToolOutput, parseToolArguments, selectTools } from "./solver";

const tools = [
  { name: "git_status", description: "status" },
  { name: "weather_forecast" },
  { name: "wikipedia_search" },
] as const;

describe("selectTools", () => {
  it("sends only the task's enabled tools in enabled scope", () => {
    const selected = selectTools(
      tools,
      ["git_status", "missing_tool"],
      "enabled"
    );
    expect(selected.definitions.map((tool) => tool.function.name)).toEqual([
      "git_status",
    ]);
    expect(selected.missingTools).toEqual(["missing_tool"]);
  });

  it("sends every sandbox tool in all scope", () => {
    const selected = selectTools(tools, ["git_status"], "all");
    expect(selected.definitions).toHaveLength(3);
    expect(selected.missingTools).toEqual([]);
  });
});

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
