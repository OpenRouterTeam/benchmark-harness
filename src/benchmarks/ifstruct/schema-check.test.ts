import { describe, expect, it } from "bun:test";

import type { JsonSchemaNode } from "./schema";
import {
  checkTopLevelStructure,
  computeExpectedChecks,
  countSchemaLeaves,
  validateAgainstJsonSchema,
} from "./schema-check";

function errorsOf(data: unknown, schema: JsonSchemaNode): string[] {
  return validateAgainstJsonSchema(data, schema)
    .filter((c) => !c.passed && c.error !== undefined)
    .map((c) => c.error ?? "");
}
describe("validateAgainstJsonSchema", () => {
  it("passes a matching object", () => {
    const schema: JsonSchemaNode = {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    };
    expect(errorsOf({ name: "ok" }, schema)).toEqual([]);
  });
  it("reports a missing required field", () => {
    const schema: JsonSchemaNode = {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    };
    expect(errorsOf({}, schema)).toContain("required field missing");
  });
  it("reports an extraneous field", () => {
    const schema: JsonSchemaNode = {
      type: "object",
      properties: { a: { type: "string" } },
    };
    expect(errorsOf({ a: "x", b: "y" }, schema)).toContain(
      "extraneous field 'b'"
    );
  });
  it("reports a wrong scalar type with a Python type name", () => {
    expect(errorsOf(5, { type: "string" })).toContain(
      "expected string, got int"
    );
  });
  it("enforces numeric minimum/maximum", () => {
    expect(errorsOf(11, { type: "integer", maximum: 10 })).toContain(
      "11 is greater than maximum 10"
    );
  });
  it("accepts null for a nullable union type", () => {
    expect(errorsOf(null, { type: ["string", "null"] })).toEqual([]);
  });
});
describe("checkTopLevelStructure", () => {
  it("leaves a bare list unwrapped when no wrapper is required", () => {
    const result = checkTopLevelStructure([1, 2], null, false);
    expect(result.wasWrapped).toBe(false);
    expect(result.error).toBeUndefined();
  });
  it("unwraps a single-key object to its inner list when a wrapper is required", () => {
    const result = checkTopLevelStructure({ items: [1, 2] }, "items", true);
    expect(result.wasWrapped).toBe(true);
    expect(result.data).toEqual([1, 2]);
    expect(result.error).toBeUndefined();
  });
  it("flags a wrapper key mismatch", () => {
    const result = checkTopLevelStructure({ things: [1] }, "items", true);
    expect(result.error).toBe("Expected top-level key 'items', got 'things'");
  });
  it("flags a bare list when a wrapper is required", () => {
    const result = checkTopLevelStructure([1], "items", true);
    expect(result.error).toBe(
      "Expected wrapped object with key 'items', got bare list"
    );
  });
});
describe("countSchemaLeaves / computeExpectedChecks", () => {
  it("counts required object leaves", () => {
    const schema: JsonSchemaNode = {
      type: "object",
      properties: { a: { type: "string" }, b: { type: "string" } },
      required: ["a", "b"],
    };
    expect(countSchemaLeaves(schema)).toBe(2);
  });
  it("multiplies per-item leaves by the fixed top-level count for arrays", () => {
    const schema: JsonSchemaNode = {
      type: "array",
      items: {
        type: "object",
        properties: { a: { type: "string" } },
        required: ["a"],
      },
    };
    expect(computeExpectedChecks(schema, 3)).toBe(3);
  });
});
