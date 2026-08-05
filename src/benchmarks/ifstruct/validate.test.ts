import { describe, expect, it } from "bun:test";

import type { IfStructRequirements, JsonSchemaNode } from "./schema";
import { OutputFormat } from "./schema";
import { validateResponse } from "./validate";

const ID_ITEM_SCHEMA: JsonSchemaNode = {
  type: "array",
  items: {
    type: "object",
    properties: { id: { type: "string" } },
    required: ["id"],
  },
  minItems: 1,
  maxItems: 1,
};

function makeRequirements(
  overrides: Partial<IfStructRequirements> = {}
): IfStructRequirements {
  return {
    jsonSchema: ID_ITEM_SCHEMA,
    topLevelCount: 1,
    topLevelKey: null,
    requireWrapperKey: false,
    requireCodeBlock: false,
    requireNoCommentary: false,
    outputFormat: OutputFormat.Json,
    ...overrides,
  };
}
describe("validateResponse", () => {
  it("passes a well-formed JSON array matching the schema", () => {
    const result = validateResponse('[{"id": "1"}]', makeRequirements());
    expect(result.passed).toBe(true);
    expect(result.score).toBe(1);
    expect(result.errors).toEqual([]);
  });
  it("fails and reports both errors when a required code block is unclosed", () => {
    const result = validateResponse(
      '```json\n[{"id": "1"}]',
      makeRequirements({ requireCodeBlock: true })
    );
    expect(result.passed).toBe(false);
    expect(result.score).toBe(0);
    expect(result.details.usesCodeBlock).toBe(false);
    expect(result.errors).toContain(
      "Response must use a code block but none was found"
    );
    expect(result.errors).toContain("Unclosed code block");
  });
  it("fails on a trailing extra brace inside a JSON code block", () => {
    const result = validateResponse(
      '```json\n[{"id": "1"}]\n}\n```',
      makeRequirements({ requireCodeBlock: true })
    );
    expect(result.passed).toBe(false);
    expect(result.details.usesCodeBlock).toBe(true);
    expect(
      result.errors.some((e) => e.includes("Trailing content after JSON"))
    ).toBe(true);
  });
  it("fails a string enum mismatch with a Python-style message", () => {
    const result = validateResponse(
      '[{"unit": "cups"}]',
      makeRequirements({
        jsonSchema: {
          type: "array",
          items: {
            type: "object",
            properties: { unit: { type: "string", enum: ["cup", "tsp"] } },
            required: ["unit"],
          },
          minItems: 1,
          maxItems: 1,
        },
      })
    );
    expect(result.passed).toBe(false);
    expect(result.errors).toContain(
      "'cups' not in allowed values ['cup', 'tsp']"
    );
  });
  it("strips thinking tags before validating", () => {
    const result = validateResponse(
      '<think>draft invalid junk</think>[{"id": "1"}]',
      makeRequirements({ requireNoCommentary: true })
    );
    expect(result.passed).toBe(true);
  });
  it("enforces an exact top-level item count", () => {
    const result = validateResponse(
      '[{"id": "1"}, {"id": "2"}]',
      makeRequirements({
        jsonSchema: { ...ID_ITEM_SCHEMA, maxItems: 2 },
        topLevelCount: 1,
      })
    );
    expect(result.passed).toBe(false);
    expect(result.errors).toContain("Expected 1 items, got 2");
  });
  it("enforces an inclusive top-level count range", () => {
    const result = validateResponse(
      '[{"id": "1"}, {"id": "2"}, {"id": "3"}]',
      makeRequirements({
        jsonSchema: { ...ID_ITEM_SCHEMA, minItems: 1, maxItems: 5 },
        topLevelCount: [1, 2],
      })
    );
    expect(result.passed).toBe(false);
    expect(result.errors).toContain("Expected 1-2 items, got 3");
  });
  it("requires a wrapper object when require_wrapper_key is set", () => {
    const result = validateResponse(
      '[{"id": "1"}]',
      makeRequirements({ requireWrapperKey: true, topLevelKey: "items" })
    );
    expect(result.passed).toBe(false);
    expect(result.errors).toContain(
      "Expected wrapped object with key 'items', got bare list"
    );
  });
  it("unwraps a single-key wrapper and validates the inner list", () => {
    const result = validateResponse(
      '{"items": [{"id": "1"}]}',
      makeRequirements({ requireWrapperKey: true, topLevelKey: "items" })
    );
    expect(result.passed).toBe(true);
    expect(result.details.wasWrapped).toBe(true);
  });
  it("validates block YAML and surfaces the schema match ratio", () => {
    const result = validateResponse(
      "```yaml\n- id: '1'\n```",
      makeRequirements({
        outputFormat: OutputFormat.Yaml,
        requireCodeBlock: true,
      })
    );
    expect(result.passed).toBe(true);
    expect(result.details.schemaMatchRatio).toBe(1);
  });
  it("rejects JSON-shaped YAML when YAML output is requested", () => {
    const result = validateResponse(
      '```yaml\n[{"id": "1"}]\n```',
      makeRequirements({ outputFormat: OutputFormat.Yaml })
    );
    expect(result.passed).toBe(false);
    expect(result.details.yamlValid).toBe(false);
  });
  it("flags commentary when require_no_commentary is set", () => {
    const result = validateResponse(
      '```json\n[{"id": "1"}]\n```\nHope this helps!',
      makeRequirements({ requireNoCommentary: true })
    );
    expect(result.passed).toBe(false);
    expect(
      result.errors.some((e) =>
        e.includes("Response contains text outside JSON")
      )
    ).toBe(true);
  });
});
