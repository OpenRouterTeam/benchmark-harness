import { describe, expect, it } from "bun:test";

import { contentBlocksToText, toToolDefinition } from "./sandbox";

describe("toToolDefinition", () => {
  it("builds a function tool with an object schema fallback", () => {
    expect(toToolDefinition({ name: "git_status" })).toEqual({
      type: "function",
      function: {
        name: "git_status",
        parameters: { type: "object", properties: {} },
      },
    });
  });

  it("keeps the MCP input schema", () => {
    const schema = { type: "object", properties: { q: { type: "string" } } };
    expect(
      toToolDefinition({
        name: "wikipedia_search",
        description: "d",
        inputSchema: schema,
      }).function
    ).toMatchObject({ description: "d", parameters: schema });
  });
});

describe("contentBlocksToText", () => {
  it("joins text blocks and labels other block types", () => {
    const text = contentBlocksToText([
      { type: "text", text: "hello" },
      { type: "image", mimeType: "image/png" },
      { type: "text", text: "world" },
    ]);
    expect(text).toContain("hello");
    expect(text).toContain("world");
    expect(text).toContain("image");
  });
});
