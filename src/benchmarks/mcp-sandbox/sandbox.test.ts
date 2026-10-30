import { describe, expect, it } from "bun:test";

import { contentBlocksToText, toDraft2020Schema, toToolDefinition } from "./sandbox";

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

describe("toDraft2020Schema", () => {
  it("rewrites draft-04 boolean exclusive bounds into the numeric form", () => {
    expect(
      toDraft2020Schema({
        type: "object",
        properties: {
          pages: {
            type: "array",
            items: { type: "integer", exclusiveMinimum: true, minimum: 0 },
          },
          ratio: { type: "number", exclusiveMaximum: true, maximum: 1 },
          count: { type: "integer", exclusiveMinimum: false, minimum: 1 },
        },
      })
    ).toEqual({
      type: "object",
      properties: {
        pages: { type: "array", items: { type: "integer", exclusiveMinimum: 0 } },
        ratio: { type: "number", exclusiveMaximum: 1 },
        count: { type: "integer", minimum: 1 },
      },
    });
  });

  it("applies when building a tool definition and leaves valid schemas alone", () => {
    const valid = { type: "object", properties: { n: { type: "integer", exclusiveMinimum: 0 } } };
    expect(toToolDefinition({ name: "t", inputSchema: valid }).function.parameters).toEqual(valid);
    expect(
      toToolDefinition({
        name: "read_pdf",
        inputSchema: { type: "object", properties: { n: { exclusiveMinimum: true, minimum: 0 } } },
      }).function.parameters
    ).toEqual({ type: "object", properties: { n: { exclusiveMinimum: 0 } } });
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
