import { describe, expect, it } from "bun:test";

import { validateJsonSchema } from "./json-schema";

const NULLABLE_NUMBER = { anyOf: [{ type: "number" }, { type: "null" }] };

describe("validateJsonSchema", () => {
  it("rejects numeric strings against anyOf [number, null]", () => {
    expect(validateJsonSchema("40", NULLABLE_NUMBER)).toEqual([
      { path: "$", message: 'string "40" matched no anyOf branch' },
    ]);
    expect(validateJsonSchema("null", NULLABLE_NUMBER)).toHaveLength(1);
    expect(validateJsonSchema(40, NULLABLE_NUMBER)).toEqual([]);
    expect(validateJsonSchema(null, NULLABLE_NUMBER)).toEqual([]);
  });

  it("handles type arrays, integers, enums and bounds", () => {
    expect(validateJsonSchema(null, { type: ["integer", "null"] })).toEqual([]);
    expect(validateJsonSchema(1.5, { type: "integer" })).toHaveLength(1);
    expect(validateJsonSchema(4, { type: "integer", enum: [1, 2, 4] })).toEqual(
      []
    );
    expect(validateJsonSchema(3, { type: "integer", enum: [1, 2, 4] })).toEqual(
      [{ path: "$", message: "number is not one of [1,2,4]" }]
    );
    expect(
      validateJsonSchema(6, { type: "integer", minimum: 1, maximum: 5 })
    ).toEqual([{ path: "$", message: "6 > maximum 5" }]);
  });

  it("reports missing required, unexpected and nested item violations", () => {
    const schema = {
      type: "object",
      additionalProperties: false,
      required: ["path", "items"],
      properties: {
        path: { type: "string" },
        items: { type: "array", items: { type: "number" } },
      },
    };
    expect(
      validateJsonSchema({ items: [1, "2"], extra: true }, schema)
    ).toEqual([
      { path: "$.path", message: "required property missing" },
      { path: "$.items[1]", message: 'expected number, got string "2"' },
      { path: "$.extra", message: "unexpected property" },
    ]);
  });
});
